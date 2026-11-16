#ifndef Version
  #error Pass /DVersion=<version>
#endif
#ifndef SourceDir
  #error Pass /DSourceDir=<folder with ngine.exe and the FFmpeg DLLs>
#endif
#ifndef OutputDir
  #error Pass /DOutputDir=<folder for the installer>
#endif

[Setup]
AppId={{6F3C1E52-7B0A-4C55-9E1D-2B8F4A6D9C31}
AppName=ngine
AppVersion={#Version}
AppVerName=ngine {#Version}
AppPublisher=ngine contributors
AppPublisherURL=https://github.com/superuserkalo/ngine
AppSupportURL=https://github.com/superuserkalo/ngine/issues
DefaultDirName={autopf}\ngine
DefaultGroupName=ngine
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19041
OutputDir={#OutputDir}
OutputBaseFilename=ngine-{#Version}-windows-x64-setup
SetupIconFile={#SourcePath}\..\icon.ico
UninstallDisplayIcon={app}\ngine.exe
UninstallDisplayName=ngine
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\ngine"; Filename: "{app}\ngine.exe"
Name: "{autodesktop}\ngine"; Filename: "{app}\ngine.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Run]
Filename: "{app}\ngine.exe"; Description: "{cm:LaunchProgram,ngine}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/im ngine.exe /f"; Flags: runhidden; RunOnceId: "StopNgine"

[UninstallDelete]
Type: files; Name: "{userstartup}\ngine.cmd"
