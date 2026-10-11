param(
    [Parameter(Mandatory = $true)]
    [string]$PackagePath,
    # Also builds the per-user Setup.exe here, signing it and its uninstaller.
    [string]$InstallerPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Stop-Signing([string]$Message) {
    throw "Windows release signing failed: $Message"
}

$requiredSecrets = @(
    "WINDOWS_CODESIGN_PFX_BASE64",
    "WINDOWS_CODESIGN_PFX_PASSWORD"
)
foreach ($name in $requiredSecrets) {
    if ([string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable($name))) {
        Stop-Signing "required GitHub Actions secret/environment variable $name is not configured"
    }
}

$resolvedPackage = (Resolve-Path -LiteralPath $PackagePath).Path
if (-not (Test-Path -LiteralPath (Join-Path $resolvedPackage "qualification.json") -PathType Leaf)) {
    Stop-Signing "package qualification.json is missing"
}
$packageRoot = Get-Item -LiteralPath $resolvedPackage -Force
if (($packageRoot.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    Stop-Signing "package directory must not be a reparse point"
}

$temporaryPfx = Join-Path $env:RUNNER_TEMP ("fframes-signing-" + [guid]::NewGuid().ToString("N") + ".pfx")
$importedThumbprints = @()
$password = $null
try {
    try {
        $encodedPfx = $env:WINDOWS_CODESIGN_PFX_BASE64
        Remove-Item Env:WINDOWS_CODESIGN_PFX_BASE64
        $pfxBytes = [Convert]::FromBase64String($encodedPfx)
        $encodedPfx = $null
    }
    catch {
        Stop-Signing "WINDOWS_CODESIGN_PFX_BASE64 is not valid base64"
    }
    if ($pfxBytes.Length -eq 0) {
        Stop-Signing "WINDOWS_CODESIGN_PFX_BASE64 decoded to an empty file"
    }
    [IO.File]::WriteAllBytes($temporaryPfx, $pfxBytes)
    [Array]::Clear($pfxBytes, 0, $pfxBytes.Length)

    $plainPassword = $env:WINDOWS_CODESIGN_PFX_PASSWORD
    Remove-Item Env:WINDOWS_CODESIGN_PFX_PASSWORD
    $password = ConvertTo-SecureString $plainPassword -AsPlainText -Force
    $plainPassword = $null
    $imported = @(Import-PfxCertificate -FilePath $temporaryPfx -CertStoreLocation Cert:\CurrentUser\My -Password $password)
    $importedThumbprints = @($imported | ForEach-Object { $_.Thumbprint } | Select-Object -Unique)
    $codeSigningOid = "1.3.6.1.5.5.7.3.3"
    $signingCertificates = @(
        $imported | Where-Object {
            $_.HasPrivateKey -and
            $_.NotBefore.ToUniversalTime() -le [DateTime]::UtcNow -and
            $_.NotAfter.ToUniversalTime() -gt [DateTime]::UtcNow -and
            ($_.EnhancedKeyUsageList | Where-Object { $_.ObjectId.Value -eq $codeSigningOid })
        }
    )
    if ($signingCertificates.Count -ne 1) {
        Stop-Signing "PFX must contain exactly one currently valid code-signing certificate with a private key"
    }
    $certificate = $signingCertificates[0]

    $signTool = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($null -eq $signTool) {
        Stop-Signing "signtool.exe is unavailable; initialize the Visual Studio/MSVC toolchain before signing"
    }
    $signToolPath = $signTool.Source
    $binaryRoot = Join-Path $resolvedPackage "bin"
    $binaries = @(
        Get-ChildItem -LiteralPath $binaryRoot -Recurse -File |
            Where-Object { $_.Extension -in @(".exe", ".dll") } |
            Sort-Object FullName
    )
    if ($binaries.Count -eq 0) {
        Stop-Signing "package contains no Windows executables or DLLs to sign"
    }
    foreach ($binary in $binaries) {
        if (($binary.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            Stop-Signing "refusing to sign a reparse-point binary: $($binary.Name)"
        }
        & $signToolPath sign /sha1 $certificate.Thumbprint /s My /fd SHA256 /tr "http://timestamp.digicert.com" /td SHA256 /d "fframes Studio" $binary.FullName
        if ($LASTEXITCODE -ne 0) {
            Stop-Signing "signtool failed to sign $($binary.Name) (exit $LASTEXITCODE)"
        }
        & $signToolPath verify /pa /all /v $binary.FullName
        if ($LASTEXITCODE -ne 0) {
            Stop-Signing "Authenticode verification failed for $($binary.Name) (exit $LASTEXITCODE)"
        }
    }

    $finalizer = Join-Path $PSScriptRoot "finalize-native-package.py"
    & python $finalizer $resolvedPackage
    if ($LASTEXITCODE -ne 0) {
        Stop-Signing "signed package inventory/archive refresh failed (exit $LASTEXITCODE)"
    }

    if (-not [string]::IsNullOrWhiteSpace($InstallerPath)) {
        # Inno Setup substitutes $q with a quote and $f with the quoted file it signs.
        $signCommand = '$q' + $signToolPath + '$q sign /sha1 ' + $certificate.Thumbprint + ' /s My /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 /d $qfframes Studio Setup$q $f'
        $installerScript = Join-Path $PSScriptRoot "package-windows-installer.py"
        & python $installerScript --source $resolvedPackage --out $InstallerPath --sign-command $signCommand
        if ($LASTEXITCODE -ne 0) {
            Stop-Signing "signed installer build failed (exit $LASTEXITCODE)"
        }
        & $signToolPath verify /pa /all /v $InstallerPath
        if ($LASTEXITCODE -ne 0) {
            Stop-Signing "Authenticode verification failed for the installer (exit $LASTEXITCODE)"
        }
    }
}
finally {
    foreach ($thumbprint in $importedThumbprints) {
        Remove-Item -LiteralPath "Cert:\CurrentUser\My\$thumbprint" -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath $temporaryPfx -Force -ErrorAction SilentlyContinue
    if ($null -ne $password) {
        $password.Dispose()
    }
}
