; BIT - Inno Setup 打包脚本
; 用法：先执行 `npm run tauri build` 生成 release 产物，
; 再用 "C:\Program Files\Inno Setup 7\ISCC.exe" installer\bit.iss 编译。

#define MyAppName "BIT"
#define MyAppVersion "0.1.4"
#define MyAppPublisher "BIT"
#define MyAppExeName "bit.exe"
; release 二进制目录（相对本 .iss 文件所在的 installer\ 目录）
#define ReleaseDir "..\src-tauri\target\release"

[Setup]
AppId={{9E2B7C4A-3F1D-4A6E-9B8C-BIT0000HUB01}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
; 输出到项目根的 installer\Output 目录
OutputDir=Output
OutputBaseFilename=BIT-Setup-{#MyAppVersion}
SetupIconFile={#ReleaseDir}\..\..\icons\icon.ico
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin

[Languages]
Name: "chinesesimplified"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#ReleaseDir}\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
; 若 release 目录含运行所需的额外资源/DLL，可按需加入下面这行（存在才拷贝）
Source: "{#ReleaseDir}\*.dll"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent
