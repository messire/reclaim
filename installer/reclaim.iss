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
{ Asks the current user's Reclaim (any copy, any version) to stop. Only processes of the
  current user can be stopped without administrator rights, which is what we want. }
procedure StopReclaim();
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM reclaim.exe /FI "USERNAME eq ' +
    GetUserNameString() + '"', '', SW_HIDE, ewWaitUntilTerminated, Code);
  { taskkill returns before the process is fully gone; give Windows a moment to release files. }
  Sleep(500);
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
procedure CurPageChanged(CurPageID: Integer);
begin
  if CurPageID = wpSelectTasks then
  begin
    if RegValueExists(HKCU, '{#RunKey}', '{#RunValue}') then
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
  { Moving to another folder: remove the old copy so only one installation exists. }
  OldDir := PreviousDir();
  NewDir := ExpandConstant('{app}');
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
