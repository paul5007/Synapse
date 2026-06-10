# Synapse Chrome Bridge

This unpacked MV3 extension lets the Synapse daemon inspect and control the
user's normal Chrome profile through a direct localhost WebSocket from the
extension service worker to the Synapse daemon. The normal end-user bridge is
tabs-first: background tab open/close/navigation use `chrome.tabs` APIs and the
extension does not require the `debugger` or `nativeMessaging` permissions.

Stable extension ID: `leoocgnkjnplbfdbklajepahofecgfbk`

Install/verify the local bridge registration with:

```powershell
scripts\install-synapse-chrome-debugger.ps1
```

Then load this directory as an unpacked extension from `chrome://extensions`.
The extension registers with the loopback daemon at `http://127.0.0.1:7700`,
then keeps an authenticated WebSocket open at `ws://127.0.0.1:7700` with a 20s
keepalive. Commands execute only after the daemon asks through the fixed
extension origin and daemon-issued bridge token. The normal bridge does not call
`runtime.connectNative()`, so Chrome does not create a native-host `cmd.exe`
wrapper on end-user systems.

Background tab commands (`openTab`, `closeTab`, and `navigateTab`) use
`chrome.tabs.create`, `chrome.tabs.remove`, `chrome.tabs.update`,
`chrome.tabs.reload`, `chrome.tabs.goBack`, and `chrome.tabs.goForward`. They do
not call `chrome.debugger.getTargets` or `chrome.debugger.attach`; target IDs
returned by this path are synthetic `chrome-tab:<tabId>` IDs backed by
`chrome.tabs` readback.

Attach-capable commands (`snapshot`, `clickNode`, `typeNode`, and `nodeValue`)
are unavailable in the normal end-user install unless a separate
debugger-enabled path is explicitly configured. Without
`--silent-debugger-extension-api`, Chrome intentionally shows its "`started
debugging this browser`" warning UI when an extension calls
`chrome.debugger.attach`. Synapse checks the target window owner PID and process
command line before attach; if the switch is absent or unreadable, Synapse
returns `A11Y_CDP_DEBUGGER_WARNING_UNSUPPRESSED` and does not call
`chrome.debugger.attach`. The extension also requires the daemon's explicit
suppression attestation on attach-capable daemon commands, so stale or malformed
daemon commands fail before `chrome.debugger.attach`.
