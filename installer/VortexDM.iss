; VortexDM — Windows Installer Script (Inno Setup 6)
; Builds a standalone Windows Setup wizard (.exe) with System Tray autostart & Uninstaller.

#define MyAppName "VortexDM"
#define MyAppVersion "1.1.0"
#define MyAppPublisher "VortexDM"
#define MyAppExeName "vortexdm.exe"
#define MyAppDesc "High-Speed Multi-Segmented Download Manager"

[Setup]
AppId={{8F6E2B3D-94B1-4E5A-A6C2-1D7C9E4F5B88}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
AllowNoIcons=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=VortexDM-Setup-{#MyAppVersion}
SetupIconFile=..\assets\vortexdm.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
UninstallDisplayName={#MyAppName} ({#MyAppDesc})
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
AppMutex=Local\VortexDM_SingleInstance_Mutex_v1
CloseApplications=force
RestartApplications=no
ChangesAssociations=yes
UsePreviousTasks=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a &Desktop shortcut"; GroupDescription: "Additional shortcuts:"
Name: "autostart"; Description: "Start {#MyAppName} minimized in the Windows &System Tray on logon"; GroupDescription: "Windows integration:"; Flags: unchecked
Name: "assoc_torrent"; Description: "Associate .&torrent files and magnet: links with {#MyAppName}"; GroupDescription: "Windows integration:"

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\vortexdm.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion isreadme

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\{#MyAppExeName}"; Comment: "{#MyAppDesc}"
Name: "{group}\Start {#MyAppName} in System Tray"; Filename: "{app}\{#MyAppExeName}"; Parameters: "--minimized"; IconFilename: "{app}\{#MyAppExeName}"
Name: "{group}\Uninstall {#MyAppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "VortexDM"; ValueData: """{app}\{#MyAppExeName}"" --minimized"; Flags: uninsdeletevalue; Tasks: autostart
Root: HKCU; Subkey: "Software\Classes\.torrent"; ValueType: string; ValueName: ""; ValueData: "VortexDM.Torrent"; Flags: uninsdeletevalue; Tasks: assoc_torrent
Root: HKCU; Subkey: "Software\Classes\.torrent\OpenWithProgids"; ValueType: string; ValueName: "VortexDM.Torrent"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc_torrent
Root: HKCU; Subkey: "Software\Classes\VortexDM.Torrent"; ValueType: string; ValueName: ""; ValueData: "BitTorrent File"; Flags: uninsdeletekey; Tasks: assoc_torrent
Root: HKCU; Subkey: "Software\Classes\VortexDM.Torrent\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\vortexdm.ico"; Tasks: assoc_torrent
Root: HKCU; Subkey: "Software\Classes\VortexDM.Torrent\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" ""%1"""; Tasks: assoc_torrent
Root: HKCU; Subkey: "Software\Classes\Applications\{#MyAppExeName}"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "{#MyAppName}"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Applications\{#MyAppExeName}\SupportedTypes"; ValueType: string; ValueName: ".torrent"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\Applications\{#MyAppExeName}\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\vortexdm.ico"
Root: HKCU; Subkey: "Software\Classes\Applications\{#MyAppExeName}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" ""%1"""
Root: HKCU; Subkey: "Software\Classes\magnet"; ValueType: string; ValueName: ""; ValueData: "URL:Magnet Protocol"; Flags: uninsdeletekey; Tasks: assoc_torrent
Root: HKCU; Subkey: "Software\Classes\magnet"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""; Tasks: assoc_torrent
Root: HKCU; Subkey: "Software\Classes\magnet\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\vortexdm.ico"; Tasks: assoc_torrent
Root: HKCU; Subkey: "Software\Classes\magnet\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" ""%1"""; Tasks: assoc_torrent

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName}"; Flags: nowait postinstall skipifsilent
