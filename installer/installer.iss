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
#define AppPublisher "Rique"

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
CloseApplications=no
RestartApplications=no
RestartIfNeededByRun=no

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: unchecked
Name: "startupicon"; Description: "Run Tapper when I sign in"; GroupDescription: "Additional shortcuts:"; Flags: unchecked

[Files]
Source: "{#PublishDir}\Tapper.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PublishDir}\tapper.settings.json"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PublishDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PublishDir}\assets\logo.png"; DestDir: "{app}\assets"; Flags: ignoreversion

[Icons]
Name: "{group}\Tapper"; Filename: "{app}\Tapper.exe"; IconFilename: "{app}\Tapper.exe"
Name: "{group}\Uninstall Tapper"; Filename: "{uninstallexe}"
Name: "{autodesktop}\Tapper"; Filename: "{app}\Tapper.exe"; IconFilename: "{app}\Tapper.exe"; Tasks: desktopicon
Name: "{userstartup}\Tapper"; Filename: "{app}\Tapper.exe"; IconFilename: "{app}\Tapper.exe"; Tasks: startupicon

[Run]
Filename: "{app}\Tapper.exe"; Description: "Launch Tapper"; Flags: nowait postinstall skipifsilent

[Code]

function EscapePowerShellLiteral(const Value: String): String;
begin
  Result := Value;
  StringChangeEx(Result, '''', '''''', True);
end;

procedure StopRunningTapper;
var
  AppPath: String;
  PowerShellPath: String;
  Parameters: String;
  ResultCode: Integer;
begin
  AppPath := ExpandConstant('{app}\Tapper.exe');
  PowerShellPath := ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe');
  Parameters :=
    '-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -Command ' +
    '"$targetPath = ''' + EscapePowerShellLiteral(AppPath) + '''; ' +
    '$targetProcesses = @(Get-Process -Name ''Tapper'' -ErrorAction SilentlyContinue | Where-Object { ' +
    'try { [string]::Equals([System.IO.Path]::GetFullPath($_.Path), [System.IO.Path]::GetFullPath($targetPath), [System.StringComparison]::OrdinalIgnoreCase) } catch { $false } ' +
    '}); ' +
    '$targetProcesses | Stop-Process -Force -ErrorAction SilentlyContinue; ' +
    '$targetProcesses | ForEach-Object { Wait-Process -Id $_.Id -Timeout 5 -ErrorAction SilentlyContinue }"';
  Exec(
    PowerShellPath,
    Parameters,
    '',
    SW_HIDE,
    ewWaitUntilTerminated,
    ResultCode);
end;

function IsPreservedInstallerFile(const Name: String): Boolean;
var
  UpperName: String;
begin
  UpperName := Uppercase(Name);
  Result := Copy(UpperName, 1, 5) = 'UNINS';
end;

procedure ClearExistingTapperAppFiles;
var
  AppDir: String;
  EntryPath: String;
  FindRec: TFindRec;
begin
  AppDir := ExpandConstant('{app}');

  if not DirExists(AppDir) then
  begin
    exit;
  end;

  if not FileExists(AppDir + '\Tapper.exe') then
  begin
    exit;
  end;

  if not FindFirst(AppDir + '\*', FindRec) then
  begin
    exit;
  end;

  try
    repeat
      if (FindRec.Name <> '.') and (FindRec.Name <> '..') and
         not IsPreservedInstallerFile(FindRec.Name) then
      begin
        EntryPath := AppDir + '\' + FindRec.Name;
        if FindRec.Attributes and FILE_ATTRIBUTE_DIRECTORY <> 0 then
        begin
          DelTree(EntryPath, True, True, True);
        end
        else
        begin
          DeleteFile(EntryPath);
        end;
      end;
    until not FindNext(FindRec);
  finally
    FindClose(FindRec);
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssInstall then
  begin
    StopRunningTapper;
    ClearExistingTapperAppFiles;
  end;
end;
