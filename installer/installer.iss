#ifndef AppVersion
  #error "AppVersion define is required."
#endif

#ifndef PublishDir
  #error "PublishDir define is required."
#endif

#ifndef OutputDir
  #error "OutputDir define is required."
#endif

#define AppName "Tapper"
#define AppPublisher "Riqqqque"

[Setup]
AppId={{6776265F-3514-4C81-89D8-BCFEA9F5207C}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={localappdata}\Tapper
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
SetupIconFile={#SourcePath}\..\assets\app-icon.ico
OutputDir={#OutputDir}
OutputBaseFilename=TapperSetup-{#AppVersion}
UninstallDisplayIcon={app}\Tapper.exe
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} Installer
SetupLogging=yes

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: unchecked
Name: "startupicon"; Description: "Run Tapper when I sign in"; GroupDescription: "Additional shortcuts:"; Flags: unchecked

[Files]
Source: "{#PublishDir}\Tapper.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PublishDir}\tapper.settings.json"; DestDir: "{app}"; Flags: onlyifdoesntexist
Source: "{#PublishDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Tapper"; Filename: "{app}\Tapper.exe"; IconFilename: "{app}\Tapper.exe"
Name: "{group}\Uninstall Tapper"; Filename: "{uninstallexe}"
Name: "{autodesktop}\Tapper"; Filename: "{app}\Tapper.exe"; IconFilename: "{app}\Tapper.exe"; Tasks: desktopicon
Name: "{userstartup}\Tapper"; Filename: "{app}\Tapper.exe"; IconFilename: "{app}\Tapper.exe"; Tasks: startupicon

[Run]
Filename: "{app}\Tapper.exe"; Description: "Launch Tapper"; Flags: nowait postinstall skipifsilent
