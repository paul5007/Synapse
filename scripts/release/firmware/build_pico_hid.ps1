param(
    [string]$Version = "",
    [string[]]$Features = @(),
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Resolve-Path (Join-Path $scriptDir "..\..\..")
$firmwareDir = Join-Path $repoRoot "firmware\pico-hid"
$cargoToml = Join-Path $firmwareDir "Cargo.toml"
$elfPath = Join-Path $firmwareDir "target\thumbv6m-none-eabi\release\pico-hid"
$featureList = @($Features | ForEach-Object { $_.Trim() } | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })

if ([string]::IsNullOrWhiteSpace($Version)) {
    $versionLine = Select-String -Path $cargoToml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if ($null -eq $versionLine) {
        throw "Could not read firmware package version from $cargoToml"
    }
    $Version = $versionLine.Matches[0].Groups[1].Value
}

$elf2uf2 = Get-Command elf2uf2-rs -ErrorAction SilentlyContinue
if ($null -eq $elf2uf2) {
    throw "elf2uf2-rs is required. Install with: cargo install elf2uf2-rs"
}

if (-not $SkipBuild) {
    Push-Location $firmwareDir
    try {
        $cargoArgs = @("build", "--release")
        if ($featureList.Count -gt 0) {
            $cargoArgs += @("--features", ($featureList -join ","))
        }
        cargo @cargoArgs
        if ($LASTEXITCODE -ne 0) {
            throw "cargo $($cargoArgs -join ' ') failed with exit code $LASTEXITCODE"
        }
    }
    finally {
        Pop-Location
    }
}

if (-not (Test-Path -LiteralPath $elfPath)) {
    throw "Firmware ELF was not found at $elfPath"
}

$outDir = Join-Path $repoRoot "scripts\release\firmware"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$featureSuffix = ""
if ($featureList.Count -gt 0) {
    $featureSuffix = "-" + (($featureList | Sort-Object) -join "-")
    $featureSuffix = $featureSuffix -replace '[^A-Za-z0-9_.-]', '-'
}
$uf2Path = Join-Path $outDir "pico-hid$featureSuffix-$Version.uf2"

& $elf2uf2.Source $elfPath $uf2Path
if ($LASTEXITCODE -ne 0) {
    throw "elf2uf2-rs failed with exit code $LASTEXITCODE"
}

$uf2 = Get-Item -LiteralPath $uf2Path
$hash = Get-FileHash -LiteralPath $uf2Path -Algorithm SHA256

[PSCustomObject]@{
    FirmwareElf = $elfPath
    Uf2Path = $uf2.FullName
    Version = $Version
    Features = if ($featureList.Count -gt 0) { $featureList -join "," } else { "default" }
    Bytes = $uf2.Length
    Sha256 = $hash.Hash
}
