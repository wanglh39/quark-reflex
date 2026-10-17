#define MyAppName "夸克助手"
#define MyAppNameEn "Quark Reflex"
#define MyAppVersion "0.1.0"
#define MyAppPublisher "Quark Reflex"
#define MyAppExeName "quark-reflex.exe"

[Setup]
AppId={{7A6F5B4C-3D2E-4F5A-9B8C-1D2E3F4A5B6C}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={localappdata}\Programs\QuarkReflex
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=dist
OutputBaseFilename=QuarkReflex-Setup-{#MyAppVersion}
SetupIconFile=assets\icon.ico
UninstallDisplayName={#MyAppName}
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible
ChangesEnvironment=no

[Languages]
Name: "chinesesimplified"; MessagesFile: "compiler:LanguagesUnofficial\ChineseSimplified.isl"

[CustomMessages]
chinesesimplified.CreateDesktopIcon=创建桌面快捷方式(&D)
chinesesimplified.LaunchProgram=运行{#MyAppName}(%1)
chinesesimplified.UninstallCredsTitle=清理本地数据
chinesesimplified.UninstallCredsText=是否同时删除本地登录凭证与配置（推荐在共用电脑上选择“是”）？

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#MyAppNameEn}}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; 卸载时清理本地配置与登录凭证（含 creds.json，避免隐私残留）
Type: filesandordirs; Name: "{userappdata}\QuarkReflex"

[Code]
function InitializeSetup(): Boolean;
var
  ResultCode: Integer;
begin
  Exec('taskkill', '/F /IM {#MyAppExeName}', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Result := True;
end;

function InitializeUninstall(): Boolean;
var
  ResultCode: Integer;
begin
  Exec('taskkill', '/F /IM {#MyAppExeName}', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Result := True;
end;