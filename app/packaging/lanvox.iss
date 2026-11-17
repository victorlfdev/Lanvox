#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef SourceBinary
  #define SourceBinary "..\target\release\lanvox.exe"
#endif
#ifndef DistributionDir
  #define DistributionDir "..\target\distribution"
#endif
#ifndef SourceReadme
  #define SourceReadme "..\target\distribution\LEIA-ME.txt"
#endif
#ifndef SourceNotices
  #define SourceNotices "..\target\distribution\THIRD_PARTY_NOTICES.txt"
#endif

[Setup]
AppId={{71C75282-E618-420E-81AD-FA310AD5E4B5}
AppName=Lanvox
AppVersion={#AppVersion}
AppPublisher=Lanvox
DefaultDirName={localappdata}\Programs\Lanvox
DefaultGroupName=Lanvox
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#DistributionDir}
OutputBaseFilename=Lanvox-{#AppVersion}-windows-x64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\lanvox.exe
SetupIconFile=..\assets\icon.ico
CloseApplications=yes
RestartApplications=no
SetupLogging=yes

[Languages]
Name: "brazilianportuguese"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Criar atalho na área de trabalho"; Flags: unchecked
Name: "autostart"; Description: "Iniciar Lanvox com o Windows"; Check: not IsLanvoxUpdate

[Files]
Source: "{#SourceBinary}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceReadme}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceNotices}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Lanvox"; Filename: "{app}\lanvox.exe"
Name: "{autodesktop}\Lanvox"; Filename: "{app}\lanvox.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Lanvox"; ValueData: """{app}\lanvox.exe"" --autostart"; Tasks: autostart; Flags: uninsdeletevalue; Check: not IsLanvoxUpdate
Root: HKCU; Subkey: "Software\Classes\lanvox"; ValueType: string; ValueName: ""; ValueData: "URL:Lanvox invitation"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\lanvox"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\lanvox\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\lanvox.exe,0"
Root: HKCU; Subkey: "Software\Classes\lanvox\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\lanvox.exe"" ""%1"""

[Run]
Filename: "{app}\lanvox.exe"; Description: "Abrir Lanvox"; Flags: nowait postinstall skipifsilent
Filename: "{app}\lanvox.exe"; Flags: nowait skipifnotsilent; Check: IsLanvoxUpdate

; Runtime data and identities intentionally live outside {app}; uninstall preserves them.

; Links of the previous name (huddle://) keep opening the app.
[Registry]
Root: HKCU; Subkey: "Software\Classes\huddle"; ValueType: string; ValueName: ""; ValueData: "URL:Lanvox invitation (legacy)"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\huddle"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\huddle\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\lanvox.exe,0"
Root: HKCU; Subkey: "Software\Classes\huddle\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\lanvox.exe"" ""%1"""

[Code]
function IsLanvoxUpdate: Boolean;
begin
  Result := ExpandConstant('{param:LANVOXUPDATE|0}') = '1';
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Lanvox');
end;
