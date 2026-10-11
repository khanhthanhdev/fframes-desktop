# Installs fframes Studio for the current user from a GitHub release:
#   irm https://github.com/khanhthanhdev/fframes-desktop/releases/latest/download/install.ps1 | iex
# With options:
#   & ([scriptblock]::Create((irm https://github.com/khanhthanhdev/fframes-desktop/releases/latest/download/install.ps1))) -Version v0.1.2 -Quiet
# Downloads the per-user Setup.exe, checks it against the release's SHA256SUMS.txt and runs it.
# No administrator rights are needed; uninstall from Settings > Apps keeps projects and app data.

function Install-FframesStudio {
    [CmdletBinding()]
    param(
        # A release tag such as v0.1.2, or "latest".
        [string]$Version = "latest",
        # No progress window; Setup runs fully silently.
        [switch]$Quiet,
        # Do not start fframes Studio after installing.
        [switch]$NoLaunch,
        [string]$Repository = "khanhthanhdev/fframes-desktop",
        # Download location override (a mirror, or a local server for testing).
        [string]$BaseUrl
    )
    $ErrorActionPreference = "Stop"
    # Invoke-WebRequest's progress bar slows large downloads down by an order of magnitude.
    $ProgressPreference = "SilentlyContinue"
    $setupName = "fframes-studio-x86_64-pc-windows-msvc-setup.exe"

    if ([Environment]::OSVersion.Version.Major -lt 10) {
        throw "fframes Studio needs Windows 10 or later."
    }
    if (-not [Environment]::Is64BitOperatingSystem) {
        throw "fframes Studio needs 64-bit Windows."
    }
    if (-not $BaseUrl) {
        $BaseUrl = if ($Version -eq "latest") {
            "https://github.com/$Repository/releases/latest/download"
        } else {
            "https://github.com/$Repository/releases/download/$Version"
        }
    }
    $BaseUrl = $BaseUrl.TrimEnd("/")
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    $work = Join-Path ([IO.Path]::GetTempPath()) ("fframes-studio-install-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $work | Out-Null
    try {
        Write-Host "Downloading checksums from $BaseUrl"
        $sumsPath = Join-Path $work "SHA256SUMS.txt"
        Invoke-WebRequest -UseBasicParsing -Uri "$BaseUrl/SHA256SUMS.txt" -OutFile $sumsPath
        $expected = $null
        foreach ($line in Get-Content -LiteralPath $sumsPath) {
            $fields = $line.Trim() -split "\s+", 2
            if ($fields.Count -eq 2 -and $fields[1].TrimStart("*") -eq $setupName) {
                $expected = $fields[0].ToUpperInvariant()
            }
        }
        if (-not $expected) {
            throw "The release has no checksum for $setupName."
        }

        Write-Host "Downloading $setupName (about 400 MB)"
        $setupPath = Join-Path $work $setupName
        Invoke-WebRequest -UseBasicParsing -Uri "$BaseUrl/$setupName" -OutFile $setupPath
        $actual = (Get-FileHash -LiteralPath $setupPath -Algorithm SHA256).Hash.ToUpperInvariant()
        if ($actual -ne $expected) {
            throw "Checksum mismatch for $setupName (expected $expected, got $actual); nothing was installed."
        }

        Write-Host "Installing fframes Studio for $env:USERNAME"
        $mode = if ($Quiet) { "/VERYSILENT" } else { "/SILENT" }
        $setup = Start-Process -FilePath $setupPath -ArgumentList $mode, "/SUPPRESSMSGBOXES", "/NORESTART" -Wait -PassThru
        if ($setup.ExitCode -ne 0) {
            throw "Setup failed with exit code $($setup.ExitCode)."
        }
    }
    finally {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }

    $key = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{A4FDB3DF-A64F-4AD1-AA24-245DE70251EC}_is1"
    $location = (Get-ItemProperty -LiteralPath $key -ErrorAction SilentlyContinue).InstallLocation
    if (-not $location) {
        throw "Setup finished but the installation was not registered."
    }
    $executable = Join-Path $location "bin\fframes-studio.exe"
    Write-Host "fframes Studio is installed in $location (Start menu: fframes Studio)."
    Write-Host "Compiling projects also needs Visual Studio Build Tools (C++) and LLVM; the app's setup screen guides you."
    if (-not $NoLaunch) {
        Start-Process -FilePath $executable | Out-Null
    }
}

Install-FframesStudio @args
