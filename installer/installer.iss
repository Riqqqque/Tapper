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
CloseApplications=no
RestartApplications=no
RestartIfNeededByRun=no

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
Filename: "{app}\Tapper.exe"; Flags: nowait

[Code]
const
  SessionManagerSubkey = 'SYSTEM\CurrentControlSet\Control\Session Manager';
  PendingRenameValueName = 'PendingFileRenameOperations';

function ReadMultiSzEntry(const Data: String; var Index: Integer): String;
var
  StartIndex: Integer;
begin
  StartIndex := Index;
  while (Index <= Length(Data)) and (Data[Index] <> #0) do
  begin
    Inc(Index);
  end;

  Result := Copy(Data, StartIndex, Index - StartIndex);
  if (Index <= Length(Data)) and (Data[Index] = #0) then
  begin
    Inc(Index);
  end;
end;

function EntryReferencesTapper(const Entry, AppPath, AppDir: String): Boolean;
var
  UpperEntry: String;
begin
  UpperEntry := Uppercase(Entry);
  Result :=
    (Pos(Uppercase(AppPath), UpperEntry) > 0) or
    (Pos(Uppercase(AppDir), UpperEntry) > 0);
end;

procedure ClearTapperPendingRenameOperations;
var
  AppPath: String;
  AppDir: String;
  OriginalData: String;
  SanitizedData: String;
  SourceEntry: String;
  TargetEntry: String;
  Index: Integer;
begin
  AppDir := ExpandConstant('{localappdata}\Tapper');
  AppPath := AppDir + '\Tapper.exe';

  if not RegQueryMultiStringValue(HKEY_LOCAL_MACHINE, SessionManagerSubkey, PendingRenameValueName, OriginalData) then
  begin
    exit;
  end;

  SanitizedData := '';
  Index := 1;

  while Index <= Length(OriginalData) do
  begin
    SourceEntry := ReadMultiSzEntry(OriginalData, Index);
    if (SourceEntry = '') and (Index > Length(OriginalData)) then
    begin
      break;
    end;

    if Index <= Length(OriginalData) then
    begin
      TargetEntry := ReadMultiSzEntry(OriginalData, Index);
    end
    else
    begin
      TargetEntry := '';
    end;

    if not EntryReferencesTapper(SourceEntry, AppPath, AppDir) and
       not EntryReferencesTapper(TargetEntry, AppPath, AppDir) then
    begin
      SanitizedData := SanitizedData + SourceEntry + #0 + TargetEntry + #0;
    end;
  end;

  if SanitizedData = OriginalData then
  begin
    exit;
  end;

  if SanitizedData = '' then
  begin
    RegDeleteValue(HKEY_LOCAL_MACHINE, SessionManagerSubkey, PendingRenameValueName);
  end
  else
  begin
    RegWriteMultiStringValue(HKEY_LOCAL_MACHINE, SessionManagerSubkey, PendingRenameValueName, SanitizedData);
  end;
end;

procedure StopRunningTapper;
var
  ResultCode: Integer;
begin
  Exec(
    ExpandConstant('{sys}\taskkill.exe'),
    '/F /T /IM Tapper.exe',
    '',
    SW_HIDE,
    ewWaitUntilTerminated,
    ResultCode);
end;

function InitializeSetup(): Boolean;
begin
  StopRunningTapper;
  ClearTapperPendingRenameOperations;
  Result := True;
end;
