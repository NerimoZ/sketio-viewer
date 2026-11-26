#ifndef AppVersion
#error Pass AppVersion with installer/build.ps1.
#endif

#define AppName "Sketio Viewer"
#define AppExe "SketioViewer.exe"
#define DocumentIcon "SketioDocument.ico"
#define CapabilitiesKey "Software\Sketio\Viewer\Capabilities"
#define MarkdownClass "Sketio.Viewer.Markdown"
#define TextClass "Sketio.Viewer.Text"
#define AppClasses "Software\Classes\Applications\" + AppExe

[Setup]
AppId=Sketio.Viewer
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Sketio
DefaultDirName={localappdata}\Programs\Sketio Viewer
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
Compression=lzma2
SolidCompression=yes
OutputDir=..\target\installer
OutputBaseFilename=Sketio-Viewer-Setup-{#AppVersion}-x64
SetupIconFile=..\ui\application.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
VersionInfoVersion={#AppVersion}.0
VersionInfoProductVersion={#AppVersion}
VersionInfoDescription={#AppName} Setup
VersionInfoCompany=Sketio
ChangesAssociations=yes
CloseApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[CustomMessages]
english.DefaultApps=Choose default file types in Windows
russian.DefaultApps=Выбрать типы файлов по умолчанию в Windows
english.DesktopShortcut=Create a desktop shortcut
russian.DesktopShortcut=Создать ярлык на рабочем столе

[Tasks]
Name: "desktopicon"; Description: "{cm:DesktopShortcut}"; Flags: unchecked

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\ui\document.ico"; DestDir: "{app}"; DestName: "{#DocumentIcon}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; AppUserModelID: "Sketio.Viewer"
Name: "{userdesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; AppUserModelID: "Sketio.Viewer"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
Filename: "ms-settings:defaultapps?registeredAppUser=Sketio%20Viewer"; Description: "{cm:DefaultApps}"; Flags: shellexec nowait postinstall skipifsilent unchecked

[Registry]
; Register candidates for Default apps and leave Windows UserChoice to the user.
Root: HKCU; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "{#AppName}"; ValueData: "{#CapabilitiesKey}"; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Sketio\Viewer"; Flags: uninsdeletekeyifempty
Root: HKCU; Subkey: "{#CapabilitiesKey}"; ValueType: string; ValueName: "ApplicationName"; ValueData: "{#AppName}"; Flags: uninsdeletekey
Root: HKCU; Subkey: "{#CapabilitiesKey}"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "View and edit Markdown and plain text files."
Root: HKCU; Subkey: "{#CapabilitiesKey}"; ValueType: string; ValueName: "ApplicationIcon"; ValueData: "{app}\{#AppExe},0"
Root: HKCU; Subkey: "{#CapabilitiesKey}\FileAssociations"; ValueType: string; ValueName: ".md"; ValueData: "{#MarkdownClass}"
Root: HKCU; Subkey: "{#CapabilitiesKey}\FileAssociations"; ValueType: string; ValueName: ".markdown"; ValueData: "{#MarkdownClass}"
Root: HKCU; Subkey: "{#CapabilitiesKey}\FileAssociations"; ValueType: string; ValueName: ".txt"; ValueData: "{#TextClass}"

Root: HKCU; Subkey: "Software\Classes\{#MarkdownClass}"; ValueType: string; ValueData: "Markdown document"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\{#MarkdownClass}\DefaultIcon"; ValueType: string; ValueData: "{app}\{#DocumentIcon},0"
Root: HKCU; Subkey: "Software\Classes\{#MarkdownClass}\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""
Root: HKCU; Subkey: "Software\Classes\{#TextClass}"; ValueType: string; ValueData: "Text document"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\{#TextClass}\DefaultIcon"; ValueType: string; ValueData: "{app}\{#DocumentIcon},0"
Root: HKCU; Subkey: "Software\Classes\{#TextClass}\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""

Root: HKCU; Subkey: "Software\Classes\.md\OpenWithProgids"; ValueType: string; ValueName: "{#MarkdownClass}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.markdown\OpenWithProgids"; ValueType: string; ValueName: "{#MarkdownClass}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.txt\OpenWithProgids"; ValueType: string; ValueName: "{#TextClass}"; ValueData: ""; Flags: uninsdeletevalue

Root: HKCU; Subkey: "{#AppClasses}"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "{#AppName}"; Flags: uninsdeletekey
Root: HKCU; Subkey: "{#AppClasses}\DefaultIcon"; ValueType: string; ValueData: "{app}\{#DocumentIcon},0"
Root: HKCU; Subkey: "{#AppClasses}\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""
Root: HKCU; Subkey: "{#AppClasses}\SupportedTypes"; ValueType: string; ValueName: ".md"; ValueData: ""
Root: HKCU; Subkey: "{#AppClasses}\SupportedTypes"; ValueType: string; ValueName: ".markdown"; ValueData: ""
Root: HKCU; Subkey: "{#AppClasses}\SupportedTypes"; ValueType: string; ValueName: ".txt"; ValueData: ""
