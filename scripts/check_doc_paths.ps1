param(
    [string]$Root
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($Root)) {
    $Root = Split-Path -Parent $PSScriptRoot
}

$root = (Resolve-Path -LiteralPath $Root).Path
$requiredDocs = @(
    @{
        Path = "docs\compressionprompt.md"
        FirstLine = "# compressionprompt.md"
    }
)

foreach ($doc in $requiredDocs) {
    $path = Join-Path $root $doc.Path
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Required documentation path is missing: $($doc.Path)"
    }

    $item = Get-Item -LiteralPath $path
    if ($item.Length -le 0) {
        throw "Required documentation path is empty: $($doc.Path)"
    }

    $firstLine = Get-Content -LiteralPath $path -TotalCount 1
    if (-not ($firstLine -like "$($doc.FirstLine)*")) {
        throw "Required documentation path has unexpected first line: $($doc.Path)"
    }
}

Write-Host "Required documentation paths verified."
