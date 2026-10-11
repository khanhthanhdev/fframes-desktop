param(
    [Parameter(Mandatory=$true)][string]$FfmpegRoot,
    [Parameter(Mandatory=$true)][string]$Out,
    [string]$Vendor
)
$ErrorActionPreference = 'Stop'
$arguments = @((Join-Path $PSScriptRoot 'assemble-phase-zero-sdk.py'), '--ffmpeg-root', $FfmpegRoot, '--out', $Out)
if ($Vendor) { $arguments += @('--vendor', $Vendor) }
# Hosts with only the Python launcher (py.exe) have no `python` on PATH.
if (Get-Command python -ErrorAction SilentlyContinue) {
    & python @arguments
} elseif (Get-Command py -ErrorAction SilentlyContinue) {
    & py -3 @arguments
} else {
    throw 'Python 3 was not found on PATH (python or py)'
}
if ($LASTEXITCODE -ne 0) { throw "SDK assembly failed with exit code $LASTEXITCODE" }
