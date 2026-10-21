; LinAmp per-user Windows installer (Inno Setup 6.3+).
; Installs for the current user only — no admin rights, no UAC prompt.
; Build with: packaging\windows\build-installer.ps1  (or ISCC linamp.iss)

#define MyAppName "LinAmp"
#define MyAppVersion "0.1.0"
#define MyAppPublisher "LinAmp contributors"
#define MyAppURL "https://github.com/cjjulius/linamp"
#define MyAppExeName "linamp.exe"
; Repo root, relative to this script (packaging\windows\).
#define RepoRoot "..\.."

[Setup]
; A stable, unique GUID identifies this app across upgrades/uninstall.
AppId={{A7E3B2C1-5D4F-4A6B-9C8E-1F2A3B4C5D6E}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
; Per-user install: never request elevation, and let a user opt into the
; current-user/all-users choice from the command line if they ever want to.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
; {autopf} resolves to the per-user Programs folder under "lowest" privileges
; (%LOCALAPPDATA%\Programs\LinAmp).
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
; Windows 10 and 11 only, 64-bit.
MinVersion=10.0
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile={#RepoRoot}\LICENSE
SetupIconFile={#RepoRoot}\assets\linamp.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
OutputDir={#RepoRoot}\dist
OutputBaseFilename=LinAmp-{#MyAppVersion}-Setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#RepoRoot}\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
; Skins must sit next to the exe — the app discovers them relative to itself.
Source: "{#RepoRoot}\skins\*"; DestDir: "{app}\skins"; Excludes: "backup\*"; Flags: ignoreversion recursesubdirs createallsubdirs
; Optional bundled demo music.
Source: "{#RepoRoot}\music\*"; DestDir: "{app}\music"; Flags: ignoreversion recursesubdirs createallsubdirs skipifsourcedoesntexist
Source: "{#RepoRoot}\README.md"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "{#RepoRoot}\FEATURES.md"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "{#RepoRoot}\SHORTCUTS.md"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "{#RepoRoot}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
; Noto Sans is embedded in the binary; carry its license.
Source: "{#RepoRoot}\assets\fonts\LICENSE-NotoSans.txt"; DestDir: "{app}\licenses"; Flags: ignoreversion skipifsourcedoesntexist

[Icons]
; WorkingDir={app} so the app's CWD-relative "skins" lookup also resolves.
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; WorkingDir: "{app}"; Flags: nowait postinstall skipifsilent
