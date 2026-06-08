<#
.SYNOPSIS
  ┌──────────────────────────────────────────────────────────────────────────┐
  │  SYNAPSE UPDATER  —  one command to get on the latest Synapse.            │
  │  Pull the newest code  ->  rebuild the binary  ->  reconnect every client │
  └──────────────────────────────────────────────────────────────────────────┘

  RUN THIS DAILY (or whenever you want the newest build). Synapse ships fixes
  and new tools frequently, so a daily run keeps your local daemon and every
  connected client (Claude Code, Codex, Claude Desktop) current with zero fuss.

      powershell -ExecutionPolicy Bypass -File .\synapse-update.ps1

.DESCRIPTION
  This is the supported "keep me up to date" entrypoint for end users. It does
  exactly three things, in order, and fails loud (never silently) if any step
  cannot complete:

    1. PULL    git fetch + fast-forward pull of this checkout from its origin
               remote, so you are building the newest committed code.
    2. BUILD   rebuild and install the native synapse-mcp.exe daemon by handing
               off to scripts/synapse-setup.ps1 (the battle-tested installer).
    3. RECONNECT  the installer also restarts the auto-start daemon and re-wires
               the Windows-side MCP clients, then verifies `health`.

  WHY THIS WRAPPER EXISTS (vs. running the installer directly):
    * The RocksDB dependency (librocksdb-sys -> bindgen) needs libclang.dll on
      PATH at BUILD time. Without it the build dies with a cryptic
      STATUS_DLL_NOT_FOUND (0xC0000135). This script auto-discovers libclang on
      ANY Windows machine — from a Visual Studio / Build Tools install (the
      bundled "C++ Clang tools for Windows" component) or a standalone LLVM —
      and puts it on PATH for the build. libclang is a BUILD-TIME-ONLY
      dependency; the running daemon never needs it.
    * It adds the `git pull` that the installer intentionally does not do.

  PORTABILITY: nothing here is machine-specific. The repo location is the
  script's own folder, Visual Studio is located via the standard `vswhere`,
  and no tokens, absolute user paths, or secrets are embedded. It is safe to
  commit and to run on any Windows 10/11 x64 system.

  PREREQUISITES (the script checks and tells you exactly what is missing):
    * git, and a Rust toolchain (rustup/cargo) — https://rustup.rs
    * A C++ build toolchain with libclang. Easiest: install "Visual Studio
      Build Tools" with the "Desktop development with C++" workload AND the
      "C++ Clang tools for Windows" component. Or install LLVM
      (winget install LLVM.LLVM) so libclang.dll exists on the machine.

.PARAMETER NoPull
  Skip the git pull and just rebuild/reconnect from the current checkout.

.PARAMETER NoPersistEnv
  Do not persist LIBCLANG_PATH to the user environment (it is still set for
  this run). By default the discovered libclang dir is saved to the USER
  environment (no admin required) so future builds find it automatically.

.PARAMETER SetupArgs
  Any remaining arguments are forwarded verbatim to scripts/synapse-setup.ps1
  (e.g. -Bind 127.0.0.1:7700, -SkipClientWiring, -Remove).

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File .\synapse-update.ps1

.EXAMPLE
  # Update the code+binary but leave client configs untouched:
  .\synapse-update.ps1 -SkipClientWiring
#>
[CmdletBinding()]
param(
    [switch]$NoPull,
    [switch]$NoPersistEnv,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$SetupArgs
)

$ErrorActionPreference = 'Stop'
function Info($m) { Write-Host "[synapse-update] $m" }
function Step($m) { Write-Host "`n=== $m ===" -ForegroundColor Cyan }
function Die($m)  { throw "[synapse-update] FATAL: $m" }

$RepoRoot   = $PSScriptRoot
$SetupScript = Join-Path $RepoRoot 'scripts\synapse-setup.ps1'

Step "Preflight"
if (-not (Test-Path $SetupScript)) {
    Die "Cannot find scripts\synapse-setup.ps1 next to this script. Run synapse-update.ps1 from inside the Synapse repo checkout."
}
foreach ($tool in 'git', 'cargo') {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        Die "'$tool' is not on PATH. Install it first (cargo: https://rustup.rs) and re-run."
    }
}
Info "repo: $RepoRoot"

# ---------------------------------------------------------------------------
# 1. PULL — fast-forward this checkout to the newest committed code.
# ---------------------------------------------------------------------------
if ($NoPull) {
    Info "Skipping git pull (-NoPull)."
} else {
    Step "Pulling latest from origin"
    if (-not (git -C $RepoRoot rev-parse --is-inside-work-tree 2>$null)) {
        Die "$RepoRoot is not a git checkout. Clone with: git clone https://github.com/ChrisRoyse/Synapse.git"
    }
    $before = (git -C $RepoRoot rev-parse --short HEAD).Trim()
    git -C $RepoRoot fetch --prune origin
    if ($LASTEXITCODE -ne 0) { Die "git fetch failed." }
    git -C $RepoRoot pull --ff-only
    if ($LASTEXITCODE -ne 0) {
        Die "git pull --ff-only failed. You likely have local commits or changes that diverge from origin. Resolve them (e.g. 'git stash' or commit on a branch) and re-run."
    }
    $after = (git -C $RepoRoot rev-parse --short HEAD).Trim()
    if ($before -eq $after) { Info "Already up to date at $after." }
    else { Info "Updated $before -> $after." }
}

# ---------------------------------------------------------------------------
# 2. libclang discovery — required on PATH for the RocksDB build script.
#    Looks (in order): existing valid LIBCLANG_PATH, Visual Studio / Build
#    Tools (via vswhere), then common standalone LLVM install locations.
# ---------------------------------------------------------------------------
function Find-LibClangDir {
    # a) Already configured and valid.
    if ($env:LIBCLANG_PATH -and (Test-Path (Join-Path $env:LIBCLANG_PATH 'libclang.dll'))) {
        return $env:LIBCLANG_PATH
    }
    # b) Visual Studio / Build Tools bundled clang, located version-agnostically.
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (Test-Path $vswhere) {
        $hits = & $vswhere -latest -prerelease -products * -find 'VC\Tools\Llvm\x64\bin\libclang.dll' 2>$null
        foreach ($hit in $hits) {
            if ($hit -and (Test-Path $hit)) { return (Split-Path $hit -Parent) }
        }
    }
    # c) Common standalone LLVM locations.
    $candidates = @(
        (Join-Path $env:ProgramFiles 'LLVM\bin\libclang.dll'),
        (Join-Path ${env:ProgramFiles(x86)} 'LLVM\bin\libclang.dll'),
        (Join-Path $env:USERPROFILE 'scoop\apps\llvm\current\bin\libclang.dll'),
        'C:\ProgramData\chocolatey\lib\llvm\tools\LLVM\bin\libclang.dll'
    )
    foreach ($c in $candidates) {
        if ($c -and (Test-Path $c)) { return (Split-Path $c -Parent) }
    }
    return $null
}

Step "Locating libclang (build-time dependency for RocksDB)"
$llvmDir = Find-LibClangDir
if (-not $llvmDir) {
    Die @"
Could not find libclang.dll on this machine. The RocksDB build needs it.
Fix it with EITHER of these, then re-run:
  * Visual Studio / Build Tools: in the Visual Studio Installer add the
    'C++ Clang tools for Windows' component (under 'Desktop development with C++').
  * Standalone LLVM:  winget install LLVM.LLVM
"@
}
Info "libclang: $llvmDir"
# Set on PATH + LIBCLANG_PATH for THIS process so the child cargo build (run by
# synapse-setup.ps1) inherits it. A freshly spawned shell does NOT inherit a
# registry edit made after that shell started, so in-process is what matters.
$env:LIBCLANG_PATH = $llvmDir
if (($env:PATH -split ';') -notcontains $llvmDir) { $env:PATH = "$llvmDir;$env:PATH" }
# Persist to the USER environment (no admin) so future builds resolve it too.
if (-not $NoPersistEnv) {
    $userLib = [Environment]::GetEnvironmentVariable('LIBCLANG_PATH', 'User')
    if ($userLib -ne $llvmDir) {
        [Environment]::SetEnvironmentVariable('LIBCLANG_PATH', $llvmDir, 'User')
        Info "Persisted LIBCLANG_PATH to user environment."
    }
}

# ---------------------------------------------------------------------------
# 3. BUILD + RECONNECT — hand off to the installer (build, install, restart the
#    auto-start daemon, re-wire MCP clients, verify health).
# ---------------------------------------------------------------------------
Step "Rebuilding and reconnecting (scripts\synapse-setup.ps1)"
$setupParams = @{ SourceDir = $RepoRoot }
& $SetupScript @setupParams @SetupArgs
if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne $null) {
    Die "synapse-setup.ps1 exited with code $LASTEXITCODE."
}

Step "Done"
Info "Synapse is updated, rebuilt, and reconnected. Run this script daily to stay current."
