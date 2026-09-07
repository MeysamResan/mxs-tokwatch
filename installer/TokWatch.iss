; Windows installer. Build locally with scripts/package.ps1.
#ifndef ProjectRoot
  #error ProjectRoot must be supplied by scripts/package.ps1
#endif
#ifndef AppVersion
  #error AppVersion must be supplied by scripts/package.ps1
#endif
#ifndef SourceExe
  #error SourceExe must be supplied by scripts/package.ps1
#endif

#ifdef TestFixture
  #define ProductName "TokWatch Installer Verification"
  #define ProductAppId "{{938A5CA4-FA7C-40F0-9EAF-E2F89746655C}"
  #define WindowClass "TokWatch.Installer.Verification.Window"
  #define AppMutexName "Local\TokWatch.Installer.Verification.v1"
  #define StartupName "TokWatchInstallerVerification"
#else
  #define ProductName "TokWatch"
  #define ProductAppId "{{A30D44CA-5373-4B32-8BFA-B7B3C2560B70}"
  #define WindowClass "TokWatch.Native.Window"
  #define AppMutexName "Local\TokWatch.Native.Tray.v1"
  #define StartupName "TokWatch"
#endif

[Setup]
AppId={#ProductAppId}
AppName={#ProductName}
AppVersion={#AppVersion}
AppPublisher=Meysam Resan
AppPublisherURL=https://github.com/MeysamResan
AppSupportURL=https://github.com/MeysamResan/mxs-tokwatch/issues
AppUpdatesURL=https://github.com/MeysamResan/mxs-tokwatch/releases
DefaultDirName={localappdata}\Programs\{#ProductName}
DefaultGroupName={#ProductName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.22000
WizardStyle=modern dark
WizardBackColor=#101010
SetupIconFile={#ProjectRoot}\installer\TokWatch.ico
UninstallDisplayIcon={app}\TokWatch.ico
UninstallDisplayName={#ProductName}
SetupMutex={#AppMutexName}.Setup
CloseApplications=no
RestartApplications=no
Compression=lzma2/max
SolidCompression=yes
TimeStampsInUTC=yes
TouchDate=2000-01-01
TouchTime=00:00:00
OutputBaseFilename=TokWatch-Setup-v{#AppVersion}-windows-x64
VersionInfoVersion={#AppVersion}.0
VersionInfoDescription=TokWatch Setup
VersionInfoProductName=TokWatch
VersionInfoProductVersion={#AppVersion}
LicenseFile={#ProjectRoot}\LICENSE

[Tasks]
Name: desktopicon; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "TokWatch.exe"; Flags: ignoreversion touch
Source: "{#ProjectRoot}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion touch
Source: "{#ProjectRoot}\installer\TokWatch.ico"; DestDir: "{app}"; Flags: ignoreversion touch

[InstallDelete]
Type: files; Name: "{app}\README.txt"

[Icons]
Name: "{group}\{#ProductName}"; Filename: "{app}\TokWatch.exe"; WorkingDir: "{app}"; IconFilename: "{app}\TokWatch.ico"; AppUserModelID: "MeysamResan.TokWatch"
Name: "{autodesktop}\{#ProductName}"; Filename: "{app}\TokWatch.exe"; WorkingDir: "{app}"; IconFilename: "{app}\TokWatch.ico"; Tasks: desktopicon; AppUserModelID: "MeysamResan.TokWatch"

[Run]
Filename: "{app}\TokWatch.exe"; WorkingDir: "{app}"; Description: "Launch TokWatch"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{app}\TokWatch.exe"; Parameters: "--uninstall-integration"; Flags: runhidden; RunOnceId: "RemoveOwnedClaudeStatusLine"

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  StartupName = '{#StartupName}';
var
  ClosedAppPath: String;

function WindowProcessId(Wnd: HWND; var ProcessId: LongWord): LongWord;
  external 'GetWindowThreadProcessId@user32.dll stdcall';
function OpenProcess(Access: LongWord; InheritHandle: Boolean; ProcessId: LongWord): THandle;
  external 'OpenProcess@kernel32.dll stdcall';
function QueryProcessImage(Process: THandle; Flags: LongWord; ImageName: String; var Size: LongWord): Boolean;
  external 'QueryFullProcessImageNameW@kernel32.dll stdcall';
function WinCloseHandle(Handle: THandle): Boolean;
  external 'CloseHandle@kernel32.dll stdcall';

function RunningAppPath(Wnd: HWND): String;
var
  ProcessId, Size: LongWord;
  Process: THandle;
begin
  Result := '';
  WindowProcessId(Wnd, ProcessId);
  Process := OpenProcess($1000, False, ProcessId);
  if Process <> 0 then begin
    Size := 32768;
    SetLength(Result, Size);
    if QueryProcessImage(Process, 0, Result, Size) then
      SetLength(Result, Size)
    else
      Result := '';
    WinCloseHandle(Process);
  end;
end;

function CloseTokWatch(): Boolean;
var
  Wnd: HWND;
  Attempt: Integer;
begin
  Result := False;
  Wnd := FindWindowByClassName('{#WindowClass}');
  if Wnd <> 0 then begin
    ClosedAppPath := RunningAppPath(Wnd);
    { WM_CLOSE only hides the tray panel. Invoke the app's normal Exit command. }
    PostMessage(Wnd, $0111, 106, 0);
  end;
  for Attempt := 1 to 100 do begin
    if (FindWindowByClassName('{#WindowClass}') = 0) and
       not CheckForMutexes('{#AppMutexName}') then begin
      Result := True;
      Exit;
    end;
    Sleep(100);
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  if not CloseTokWatch() then
    Result := 'TokWatch is still running. Exit it from its tray menu, then try again.';
end;

function StartupPointsTo(const Value, FileName: String): Boolean;
begin
  Result := (FileName <> '') and
    ((CompareText(Trim(Value), '"' + FileName + '"') = 0) or
     (CompareText(Trim(Value), FileName) = 0));
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Value, InstalledExe: String;
begin
  if CurStep = ssPostInstall then begin
    InstalledExe := ExpandConstant('{app}\TokWatch.exe');
    if RegQueryStringValue(HKCU, RunKey, StartupName, Value) and
       (StartupPointsTo(Value, ClosedAppPath) or StartupPointsTo(Value, InstalledExe)) then
      RegWriteStringValue(HKCU, RunKey, StartupName, '"' + InstalledExe + '"');
  end;
end;

function InitializeUninstall(): Boolean;
begin
  Result := CloseTokWatch();
  if not Result then
    SuppressibleMsgBox('TokWatch is still running. Exit it from its tray menu, then uninstall again.', mbError, MB_OK, IDOK);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Value: String;
begin
  if CurUninstallStep = usUninstall then begin
    if RegQueryStringValue(HKCU, RunKey, StartupName, Value) and
       StartupPointsTo(Value, ExpandConstant('{app}\TokWatch.exe')) then
      RegDeleteValue(HKCU, RunKey, StartupName);
  end;
  { User settings and provider-owned authentication are deliberately outside [Files]. }
end;
