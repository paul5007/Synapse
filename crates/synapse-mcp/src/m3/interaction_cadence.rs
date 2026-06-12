//! Counts-only low-level input hook for timeline interaction cadence (#838).
//!
//! The hook never records key names, characters, mouse coordinates, window
//! text, or clipboard content. It only emits event class counters plus the
//! OS-injected flag so the activity recorder can keep human cadence separate
//! from Synapse-generated input.

use anyhow::Result;
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InteractionEvent {
    pub ts_ns: u64,
    pub kind: InteractionEventKind,
    pub injected: bool,
    pub key_signal: Option<InteractionKeySignal>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionEventKind {
    Keystroke,
    Click,
    VerticalScroll { delta: i32 },
    HorizontalScroll { delta: i32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionKeySignal {
    UndoCommand,
    DeleteCommand,
    TextLikeKey,
    OtherKey,
}

pub struct InteractionHook {
    inner: platform::InteractionHook,
}

impl InteractionHook {
    /// Starts the platform low-level input hook.
    ///
    /// # Errors
    ///
    /// Returns an error if the platform hook cannot be installed. The daemon
    /// must fail closed rather than silently run without cadence rows.
    pub fn start(sender: mpsc::UnboundedSender<InteractionEvent>) -> Result<Self> {
        Ok(Self {
            inner: platform::InteractionHook::start(sender)?,
        })
    }

    #[must_use]
    pub const fn readback(&self) -> &InteractionHookReadback {
        self.inner.readback()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteractionHookReadback {
    pub thread_id: u32,
    pub keyboard_hook_installed: bool,
    pub mouse_hook_installed: bool,
}

#[cfg(windows)]
mod platform {
    use std::{
        sync::{Mutex, OnceLock, mpsc as std_mpsc},
        thread,
    };

    use anyhow::{Context, Result, bail};
    use tokio::sync::mpsc;
    use windows::Win32::{
        Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM},
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::Input::KeyboardAndMouse::GetAsyncKeyState,
        UI::WindowsAndMessaging::{
            CallNextHookEx, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
            PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL,
            WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_MOUSEHWHEEL,
            WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN,
        },
    };

    use super::{
        InteractionEvent, InteractionEventKind, InteractionHookReadback, InteractionKeySignal,
    };

    const LLKHF_INJECTED_MASK: u32 = 0x12;
    const LLMHF_INJECTED_MASK: u32 = 0x03;
    const KEY_DOWN_MASK: u16 = 0x8000;
    const VK_BACK_CODE: u32 = 0x08;
    const VK_SPACE_CODE: u32 = 0x20;
    const VK_DELETE_CODE: u32 = 0x2e;
    const VK_CONTROL_CODE: u32 = 0x11;
    const VK_LCONTROL_CODE: u32 = 0xa2;
    const VK_RCONTROL_CODE: u32 = 0xa3;
    const VK_Z_CODE: u32 = 0x5a;
    const VK_PACKET_CODE: u32 = 0xe7;

    static HOOK_SENDER: OnceLock<Mutex<Option<mpsc::UnboundedSender<InteractionEvent>>>> =
        OnceLock::new();

    pub struct InteractionHook {
        readback: InteractionHookReadback,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl InteractionHook {
        pub fn start(sender: mpsc::UnboundedSender<InteractionEvent>) -> Result<Self> {
            {
                let mut slot = hook_sender()
                    .lock()
                    .map_err(|_| anyhow::anyhow!("interaction hook sender lock poisoned"))?;
                if slot.is_some() {
                    bail!("interaction cadence hook is already installed in this process");
                }
                *slot = Some(sender);
            }

            let (ready_tx, ready_rx) = std_mpsc::channel();
            let thread = thread::Builder::new()
                .name("synapse-interaction-cadence-hook".to_owned())
                .spawn(move || run_hook_thread(ready_tx))
                .context("spawn interaction cadence hook thread")?;
            let readback = match ready_rx.recv() {
                Ok(Ok(readback)) => readback,
                Ok(Err(error)) => {
                    clear_sender();
                    let _ = thread.join();
                    bail!(error);
                }
                Err(error) => {
                    clear_sender();
                    let _ = thread.join();
                    bail!("interaction cadence hook thread exited before readiness: {error}");
                }
            };
            Ok(Self {
                readback,
                thread: Some(thread),
            })
        }

        pub const fn readback(&self) -> &InteractionHookReadback {
            &self.readback
        }
    }

    impl Drop for InteractionHook {
        fn drop(&mut self) {
            let _ = unsafe {
                PostThreadMessageW(self.readback.thread_id, WM_QUIT, WPARAM(0), LPARAM(0))
            };
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            clear_sender();
        }
    }

    struct HookGuard(HHOOK);

    impl Drop for HookGuard {
        fn drop(&mut self) {
            let _ = unsafe { UnhookWindowsHookEx(self.0) };
        }
    }

    fn hook_sender() -> &'static Mutex<Option<mpsc::UnboundedSender<InteractionEvent>>> {
        HOOK_SENDER.get_or_init(|| Mutex::new(None))
    }

    fn clear_sender() {
        if let Ok(mut guard) = hook_sender().lock() {
            *guard = None;
        }
    }

    fn run_hook_thread(ready_tx: std_mpsc::Sender<Result<InteractionHookReadback, String>>) {
        let thread_id = unsafe { GetCurrentThreadId() };
        let module = match unsafe { GetModuleHandleW(None) } {
            Ok(module) => module,
            Err(error) => {
                let _ = ready_tx.send(Err(format!(
                    "GetModuleHandleW failed for interaction cadence hook: {error}"
                )));
                return;
            }
        };
        let keyboard_hook = match unsafe {
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(keyboard_hook_proc),
                Some(HINSTANCE(module.0)),
                0,
            )
        } {
            Ok(hook) => hook,
            Err(error) => {
                let _ = ready_tx.send(Err(format!(
                    "SetWindowsHookExW(WH_KEYBOARD_LL) failed: {error}"
                )));
                return;
            }
        };
        let mouse_hook = match unsafe {
            SetWindowsHookExW(
                WH_MOUSE_LL,
                Some(mouse_hook_proc),
                Some(HINSTANCE(module.0)),
                0,
            )
        } {
            Ok(hook) => hook,
            Err(error) => {
                let _keyboard_guard = HookGuard(keyboard_hook);
                let _ = ready_tx.send(Err(format!(
                    "SetWindowsHookExW(WH_MOUSE_LL) failed: {error}"
                )));
                return;
            }
        };
        let _keyboard_guard = HookGuard(keyboard_hook);
        let _mouse_guard = HookGuard(mouse_hook);
        let _ = ready_tx.send(Ok(InteractionHookReadback {
            thread_id,
            keyboard_hook_installed: true,
            mouse_hook_installed: true,
        }));

        let mut msg = MSG::default();
        while unsafe { GetMessageW(&raw mut msg, None, 0, 0).as_bool() } {}
    }

    unsafe extern "system" fn keyboard_hook_proc(
        code: i32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if code >= 0 {
            let message = u32::try_from(wparam.0).unwrap_or(0);
            if matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN) {
                let data = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
                emit(
                    InteractionEventKind::Keystroke,
                    data.flags.0 & LLKHF_INJECTED_MASK != 0,
                    Some(key_signal(data.vkCode)),
                );
            } else if matches!(message, WM_KEYUP | WM_SYSKEYUP) {
                // Key-up confirms release state but is not an interaction
                // count. Counting key-down only avoids doubling keystrokes.
            }
        }
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    unsafe extern "system" fn mouse_hook_proc(
        code: i32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if code >= 0 {
            let message = u32::try_from(wparam.0).unwrap_or(0);
            let data = unsafe { *(lparam.0 as *const MSLLHOOKSTRUCT) };
            let injected = data.flags & LLMHF_INJECTED_MASK != 0;
            match message {
                WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN => {
                    emit(InteractionEventKind::Click, injected, None);
                }
                WM_MOUSEWHEEL => {
                    emit(
                        InteractionEventKind::VerticalScroll {
                            delta: wheel_delta(data.mouseData),
                        },
                        injected,
                        None,
                    );
                }
                WM_MOUSEHWHEEL => {
                    emit(
                        InteractionEventKind::HorizontalScroll {
                            delta: wheel_delta(data.mouseData),
                        },
                        injected,
                        None,
                    );
                }
                _ => {}
            }
        }
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    fn emit(kind: InteractionEventKind, injected: bool, key_signal: Option<InteractionKeySignal>) {
        let event = InteractionEvent {
            ts_ns: super::super_now_ts_ns(),
            kind,
            injected,
            key_signal,
        };
        if let Ok(guard) = hook_sender().lock()
            && let Some(sender) = guard.as_ref()
        {
            let _ = sender.send(event);
        }
    }

    fn wheel_delta(mouse_data: u32) -> i32 {
        i32::from(((mouse_data >> 16) as u16) as i16)
    }

    fn key_signal(vk_code: u32) -> InteractionKeySignal {
        key_signal_with_ctrl(vk_code, ctrl_down())
    }

    fn key_signal_with_ctrl(vk_code: u32, ctrl_down: bool) -> InteractionKeySignal {
        if vk_code == VK_Z_CODE && ctrl_down {
            return InteractionKeySignal::UndoCommand;
        }
        if matches!(vk_code, VK_BACK_CODE | VK_DELETE_CODE) {
            return InteractionKeySignal::DeleteCommand;
        }
        if text_like_key(vk_code) {
            InteractionKeySignal::TextLikeKey
        } else {
            InteractionKeySignal::OtherKey
        }
    }

    fn ctrl_down() -> bool {
        key_down(VK_CONTROL_CODE) || key_down(VK_LCONTROL_CODE) || key_down(VK_RCONTROL_CODE)
    }

    fn key_down(vk_code: u32) -> bool {
        let state = unsafe { GetAsyncKeyState(i32::try_from(vk_code).unwrap_or(0)) };
        (state as u16 & KEY_DOWN_MASK) != 0
    }

    const fn text_like_key(vk_code: u32) -> bool {
        matches!(
            vk_code,
            VK_SPACE_CODE
                | VK_PACKET_CODE
                | 0x30..=0x39
                | 0x41..=0x5a
                | 0x60..=0x6f
                | 0xba..=0xc0
                | 0xdb..=0xdf
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn unicode_sendinput_packet_is_text_like_without_raw_character() {
            assert_eq!(
                key_signal_with_ctrl(VK_PACKET_CODE, false),
                InteractionKeySignal::TextLikeKey
            );
            assert_eq!(
                key_signal_with_ctrl(VK_BACK_CODE, false),
                InteractionKeySignal::DeleteCommand
            );
            assert_eq!(
                key_signal_with_ctrl(VK_Z_CODE, true),
                InteractionKeySignal::UndoCommand
            );
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use anyhow::{Result, bail};
    use tokio::sync::mpsc;

    use super::{InteractionEvent, InteractionHookReadback};

    pub struct InteractionHook {
        readback: InteractionHookReadback,
    }

    impl InteractionHook {
        pub fn start(_sender: mpsc::UnboundedSender<InteractionEvent>) -> Result<Self> {
            bail!("interaction cadence hook requires Windows")
        }

        pub const fn readback(&self) -> &InteractionHookReadback {
            &self.readback
        }
    }
}

fn super_now_ts_ns() -> u64 {
    let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(i64::MAX);
    u64::try_from(nanos).unwrap_or(0)
}
