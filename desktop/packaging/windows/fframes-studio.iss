; Per-user Windows installer for the native package (desktop/scripts/package-phase-zero.py).
; Built by desktop/scripts/package-windows-installer.py, which passes the defines below.
; Installs without administrator rights into %LOCALAPPDATA%\Programs\fframes Studio.
; Uninstall removes only that folder and the shortcuts: projects (%USERPROFILE%\.fframes),
; the managed SDK and app state (%LOCALAPPDATA%\fframes-studio) are kept.

#ifndef AppVersion
  #error AppVersion must be defined
#endif
#ifndef VersionInfo
  #error VersionInfo must be defined
#endif
#ifndef SourceDir
  #error SourceDir must be defined
#endif
#ifndef OutputDir
  #error OutputDir must be defined
#endif
#ifndef OutputBaseFilename
  #error OutputBaseFilename must be defined
#endif

[Setup]
; Never change AppId: upgrades and the uninstaller find the existing install by it.
AppId={{A4FDB3DF-A64F-4AD1-AA24-245DE70251EC}
AppName=fframes Studio
AppVersion={#AppVersion}
AppVerName=fframes Studio {#AppVersion}
AppPublisher=khanhthanhdev
AppPublisherURL=https://github.com/khanhthanhdev/fframes-desktop
VersionInfoVersion={#VersionInfo}
VersionInfoProductName=fframes Studio
VersionInfoDescription=fframes Studio Setup
PrivilegesRequired=lowest
DefaultDirName={autopf}\fframes Studio
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
DisableWelcomePage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
WizardStyle=modern dynamic
SetupIconFile=fframes-studio.ico
UninstallDisplayIcon={app}\fframes-studio.ico
UninstallDisplayName=fframes Studio
CloseApplications=yes
RestartApplications=no
Compression=lzma2/max
SolidCompression=yes
OutputDir={#OutputDir}
OutputBaseFilename={#OutputBaseFilename}
#ifdef SignTool
SignTool=release
SignedUninstaller=yes
#endif

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[InstallDelete]
; An upgrade replaces the previous version's files instead of leaving stale ones behind.
Type: filesandordirs; Name: "{app}\bin"
Type: filesandordirs; Name: "{app}\sdk"
Type: filesandordirs; Name: "{app}\worker-source"
Type: filesandordirs; Name: "{app}\notices"

[Files]
; The shortcuts start the executable directly; it finds sdk\ beside bin\ on its own.
Source: "{#SourceDir}\*"; DestDir: "{app}"; Excludes: "\launch.bat,\launch.ps1"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "fframes-studio.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\fframes Studio"; Filename: "{app}\bin\fframes-studio.exe"; WorkingDir: "{app}"; IconFilename: "{app}\fframes-studio.ico"
Name: "{autodesktop}\fframes Studio"; Filename: "{app}\bin\fframes-studio.exe"; WorkingDir: "{app}"; IconFilename: "{app}\fframes-studio.ico"

[Run]
Filename: "{app}\bin\fframes-studio.exe"; WorkingDir: "{app}"; Description: "{cm:LaunchProgram,fframes Studio}"; Flags: nowait postinstall skipifsilent
