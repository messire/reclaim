; Reclaim installer (Inno Setup 6). Build it with scripts\build-installer.cmd, which
; compiles the binaries and passes AppVersion, StageDir and OutputDir; see installer/README.md.
;
; One application identity for every version: AppId below never changes. A newer setup
; finds the existing installation by it, closes the running app, and replaces the files in
; place, so versions are never installed side by side. If the user picks a different folder
; during an upgrade, the old copy is uninstalled first.
;
; Per-user install (no administrator rights): %LOCALAPPDATA%\Programs\Reclaim by default.

#ifndef AppVersion
  #error AppVersion is not defined: build with scripts\build-installer.cmd
#endif
#ifndef StageDir
  #error StageDir is not defined: build with scripts\build-installer.cmd
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

; NEVER change this GUID: it is how a new version recognises the installed one.
#define AppGuid "09543C3B-C0B8-43FA-BC08-E11A6FB50CD4"
#define UninstallKey "Software\Microsoft\Windows\CurrentVersion\Uninstall\{" + AppGuid + "}_is1"
; Shared with the app (crates/reclaim-app/src/ui/autostart.rs).
#define RunKey "Software\Microsoft\Windows\CurrentVersion\Run"
#define ApprovedKey "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"
#define RunValue "Reclaim"

[Setup]
AppId={{{#AppGuid}}
AppName=Reclaim
AppVersion={#AppVersion}
AppVerName=Reclaim {#AppVersion}
AppPublisher=Reclaim
AppPublisherURL=https://github.com/messire/reclaim
AppSupportURL=https://github.com/messire/reclaim/issues
VersionInfoVersion={#AppVersion}
VersionInfoProductName=Reclaim
DefaultDirName={autopf}\Reclaim
DefaultGroupName=Reclaim
DisableProgramGroupPage=yes
; Always offer the folder; on an upgrade it is pre-filled with the current one.
DisableDirPage=no
UsePreviousAppDir=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Windows 10 22H2 (build 19045) and later.
MinVersion=10.0.19045
SetupIconFile=..\crates\reclaim-app\assets\icons\app.ico
UninstallDisplayIcon={app}\reclaim.exe
UninstallDisplayName=Reclaim
LicenseFile={#StageDir}\LICENSE
OutputDir={#OutputDir}
OutputBaseFilename=reclaim-{#AppVersion}-windows-x64-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Reclaim is closed before files are replaced (see PrepareToInstall); this is the fallback.
CloseApplications=force
RestartApplications=no
ShowLanguageDialog=auto

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[CustomMessages]
english.AutostartTask=Start Reclaim with Windows (in the tray)
russian.AutostartTask=Запускать Reclaim вместе с Windows (в трее)
english.DesktopTask=Create a desktop shortcut
russian.DesktopTask=Создать ярлык на рабочем столе
english.Options=Options:
russian.Options=Параметры:
english.LaunchApp=Launch Reclaim
russian.LaunchApp=Запустить Reclaim
english.StillRunning=Reclaim is still running and setup cannot close it, probably because it was started as administrator. Close it (tray icon → Exit, or end reclaim.exe in Task Manager), then click Back and Next.
russian.StillRunning=Reclaim всё ещё запущен, и установщик не может его закрыть — вероятно, он запущен от имени администратора. Закройте его (значок в трее → Exit или завершите reclaim.exe в диспетчере задач), затем нажмите «Назад» и «Далее».

[Tasks]
Name: "autostart"; Description: "{cm:AutostartTask}"; GroupDescription: "{cm:Options}"; Flags: unchecked
Name: "desktopicon"; Description: "{cm:DesktopTask}"; GroupDescription: "{cm:Options}"; Flags: unchecked

[Files]
Source: "{#StageDir}\reclaim.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\reclaim-cli.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\THIRD_PARTY_NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Reclaim"; Filename: "{app}\reclaim.exe"
Name: "{autodesktop}\Reclaim"; Filename: "{app}\reclaim.exe"; Tasks: desktopicon

[Registry]
; Start with Windows: the same value the app's Settings checkbox writes. Applied at install
; time; unticking it on an upgrade removes it. Uninstall always removes it.
Root: HKCU; Subkey: "{#RunKey}"; ValueType: string; ValueName: "{#RunValue}"; ValueData: """{app}\reclaim.exe"" --tray"; Tasks: autostart; Flags: uninsdeletevalue
Root: HKCU; Subkey: "{#RunKey}"; ValueType: none; ValueName: "{#RunValue}"; Tasks: not autostart; Flags: deletevalue uninsdeletevalue
; Clears "turned off in Task Manager", so a ticked box really means on.
Root: HKCU; Subkey: "{#ApprovedKey}"; ValueType: none; ValueName: "{#RunValue}"; Flags: deletevalue uninsdeletevalue

[Run]
Filename: "{app}\reclaim.exe"; Description: "{cm:LaunchApp}"; Flags: nowait postinstall skipifsilent

[Code]
const
  GENERIC_WRITE = $40000000;
  OPEN_EXISTING = 3;
  INVALID_HANDLE_VALUE = -1;

function CreateFileW(Name: String; Access, Share, Security, Disposition, Flags,
  Template: Cardinal): Integer; external 'CreateFileW@kernel32.dll stdcall';
function CloseHandle(Handle: Integer): Boolean; external 'CloseHandle@kernel32.dll stdcall';

{ True when the file is missing or can be opened for writing. A running exe cannot: Windows
  keeps its image locked until the process is gone. }
function Released(const Path: String): Boolean;
var
  H: Integer;
begin
  Result := not FileExists(Path);
  if not Result then
  begin
    H := CreateFileW(Path, GENERIC_WRITE, 0, 0, OPEN_EXISTING, 0, 0);
    Result := H <> INVALID_HANDLE_VALUE;
    if Result then
      CloseHandle(H);
  end;
end;

{ Asks the current user's Reclaim (any copy, any version) to stop. Only processes of the
  current user can be stopped without administrator rights; a copy started as administrator
  survives, which Released() then reports. }
procedure StopReclaim();
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM reclaim.exe /FI "USERNAME eq ' +
    GetUserNameString() + '"', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

{ taskkill returns before the process is fully gone: wait up to 10 s for the exe to be free. }
function WaitReleased(const Path: String): Boolean;
var
  I: Integer;
begin
  Result := Released(Path);
  I := 0;
  while (not Result) and (I < 40) do
  begin
    Sleep(250);
    Result := Released(Path);
    I := I + 1;
  end;
end;

function PreviousDir(): String;
begin
  if not RegQueryStringValue(HKCU, '{#UninstallKey}', 'InstallLocation', Result) then
    Result := '';
end;

function PreviousUninstaller(): String;
begin
  if not RegQueryStringValue(HKCU, '{#UninstallKey}', 'UninstallString', Result) then
    Result := '';
  Result := RemoveQuotes(Result);
end;

{ The tasks page shows the real start-with-Windows state, which the app's Settings may
  have changed since the last install. }
function AutostartOn(): Boolean;
var
  Flag: AnsiString;
begin
  Result := RegValueExists(HKCU, '{#RunKey}', '{#RunValue}');
  { Turned off in Task Manager / Settings > Apps > Startup: the first byte is odd. }
  if Result and RegQueryBinaryValue(HKCU, '{#ApprovedKey}', '{#RunValue}', Flag) and
     (Length(Flag) > 0) and ((Ord(Flag[1]) and 1) = 1) then
    Result := False;
end;

{ Not in silent mode: the pages are still stepped through there, and this would
  override /TASKS. }
procedure CurPageChanged(CurPageID: Integer);
begin
  if (CurPageID = wpSelectTasks) and not WizardSilent() then
  begin
    if AutostartOn() then
      WizardSelectTasks('autostart')
    else
      WizardSelectTasks('!autostart');
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  OldDir, NewDir, Uninstaller: String;
  Code: Integer;
begin
  Result := '';
  StopReclaim();
  OldDir := PreviousDir();
  NewDir := ExpandConstant('{app}');
  if not WaitReleased(AddBackslash(NewDir) + 'reclaim.exe') or
     ((OldDir <> '') and not WaitReleased(AddBackslash(OldDir) + 'reclaim.exe')) then
  begin
    Result := CustomMessage('StillRunning');
    Exit;
  end;
  { Moving to another folder: remove the old copy so only one installation exists. }
  Uninstaller := PreviousUninstaller();
  if (OldDir <> '') and (Uninstaller <> '') and FileExists(Uninstaller) and
     (CompareText(RemoveBackslashUnlessRoot(OldDir), RemoveBackslashUnlessRoot(NewDir)) <> 0) then
  begin
    if not Exec(Uninstaller, '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART', '', SW_HIDE,
                ewWaitUntilTerminated, Code) or (Code <> 0) then
      Result := 'The previous installation in ' + OldDir + ' could not be removed (code ' +
        IntToStr(Code) + '). Uninstall it from Settings → Apps, then run this setup again.';
  end;
end;

function InitializeUninstall(): Boolean;
begin
  StopReclaim();
  Result := True;
end;
