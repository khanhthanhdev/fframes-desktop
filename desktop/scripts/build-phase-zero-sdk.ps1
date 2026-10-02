param(
    [Parameter(Mandatory=$true)][string]$FfmpegRoot,
    [Parameter(Mandatory=$true)][string]$Out,
    [string]$Vendor
)
$ErrorActionPreference = 'Stop'
$arguments = @((Join-Path $PSScriptRoot 'assemble-phase-zero-sdk.py'), '--ffmpeg-root', $FfmpegRoot, '--out', $Out)
if ($Vendor) { $arguments += @('--vendor', $Vendor) }
& python @arguments
if ($LASTEXITCODE -ne 0) { throw "SDK assembly failed with exit code $LASTEXITCODE" }
