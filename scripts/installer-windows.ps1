param([string]$SetupPath,[string]$DataDir,[string]$LegacyFixture)
. (Join-Path $PSScriptRoot 'windows-test-helpers.ps1')
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;using System.Runtime.InteropServices;
public static class InstallerTestNative {
 [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr context);
}
'@
[InstallerTestNative]::SetProcessDpiAwarenessContext([IntPtr](-4))|Out-Null
function Pause([int]$ms=250){$w=[Diagnostics.Stopwatch]::StartNew();while($w.ElapsedMilliseconds -lt $ms){[Windows.Forms.Application]::DoEvents();Start-Sleep -Milliseconds 10}}
function Hash([string]$path){return (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash}
function RunSetup([int]$expected=0){
 $job=Start-Process -FilePath $SetupPath -ArgumentList ('/S /D='+$install) -Wait -PassThru
 Assert ($job.ExitCode -eq $expected) "Installer exit $($job.ExitCode), expected $expected"
}
function Uninstall{
 $job=Start-Process -FilePath (Join-Path $install 'Uninstall.exe') -ArgumentList '/S' -Wait -PassThru
 Assert ($job.ExitCode -eq 0) "Uninstall exit $($job.ExitCode)"
 for($i=0;$i -lt 100 -and (Test-Path (Join-Path $install 'Keyisle.exe'));$i++){Pause 100}
 Assert (-not (Test-Path (Join-Path $install 'Keyisle.exe'))) 'Uninstall left executable'
}
function CaptureWizard($process,[string]$name){
 $condition=[Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty,$process.Id)
 $window=$null
 for($i=0;$i -lt 50;$i++){Pause 100;$window=[Windows.Automation.AutomationElement]::RootElement.FindFirst([Windows.Automation.TreeScope]::Children,$condition);if($window){break}}
 Assert ($null -ne $window) 'Installer wizard missing'
 $handle=[IntPtr]$window.Current.NativeWindowHandle
 [VaultTestNative]::SetForegroundWindow($handle)|Out-Null;Pause 500
 Assert ([VaultTestNative]::GetForegroundWindow() -eq $handle) 'Installer wizard is not foreground'
 for($i=0;$i -lt 50 -and @(Visible $window ([Windows.Automation.AutomationElement]::NameProperty) '安装 Keyisle').Count -eq 0;$i++){Pause 100}
 Assert (@(Visible $window ([Windows.Automation.AutomationElement]::NameProperty) '安装 Keyisle').Count -gt 0) 'Branded installer welcome text missing'
 $r=$window.Current.BoundingRectangle;$b=[Drawing.Bitmap]::new([int]$r.Width,[int]$r.Height);$g=[Drawing.Graphics]::FromImage($b)
 try{$g.CopyFromScreen([int]$r.Left,[int]$r.Top,0,0,$b.Size);$b.Save((Join-Path $DataDir $name),[Drawing.Imaging.ImageFormat]::Png)}finally{$g.Dispose();$b.Dispose()}
}
Assert ($SetupPath -like '*-test.exe') 'Use only isolated test installer'
Assert ($DataDir -match 'test-v1\.0\.0-') 'Use isolated test root'
Assert ([VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents') -eq [IntPtr]::Zero) 'Close existing app first'
$registry='HKCU:\Software\KeyisleInstallerTests'
Assert (-not (Test-Path $registry)) 'Prior installer test registry exists; do not mix runs'
New-Item -ItemType Directory -Force $DataDir|Out-Null
$install=Join-Path $DataDir '程序 目录';$vaultDir=Join-Path $DataDir 'data'
New-Item -ItemType Directory -Force $vaultDir|Out-Null
Assert ($LegacyFixture -match 'diagnostics\\v0\.12\.0\\test-v0\.12\.0-') 'Legacy source must be an archived isolated test fixture'
Assert (-not (Test-Path (Join-Path $vaultDir 'vault.db'))) 'Use a fresh installer test directory'
Assert (Test-Path (Join-Path $LegacyFixture 'vault.db')) 'Requires an isolated archived 0.12 fixture'
Copy-Item -LiteralPath (Join-Path $LegacyFixture 'vault.db') -Destination (Join-Path $vaultDir 'vault.db')
foreach($name in @('lock-settings.txt','theme.txt','appearance.txt','shortcut.txt')){if(Test-Path (Join-Path $LegacyFixture $name)){Copy-Item -LiteralPath (Join-Path $LegacyFixture $name) -Destination (Join-Path $vaultDir $name)}}
$env:PASSWORD_MANAGER_DATA_DIR=$vaultDir;$env:PASSWORD_MANAGER_TEST_RUN_KEY='Software\PasswordHelperTests\InstallerRun'
$testRun='HKCU:\Software\PasswordHelperTests\InstallerRun'
Assert (-not (Test-Path $testRun)) 'Prior isolated Run key exists'
$script:process=$null;$wizard=$null;$ExePath=Join-Path $install 'Keyisle.exe'
$realPath='HKCU:\Software\Microsoft\Windows\CurrentVersion\Run';$realBefore=(Get-ItemProperty -LiteralPath $realPath -Name WindowsPasswordManager -ErrorAction SilentlyContinue).WindowsPasswordManager
$realProductBefore=Test-Path 'HKCU:\Software\Keyisle'
trap{Write-Output $_.Exception.Message;Write-Output $_.ScriptStackTrace;throw}
try{
 $wizard=Start-Process -FilePath $SetupPath -PassThru;CaptureWizard $wizard '01-installer-welcome.png';Stop-Process -Id $wizard.Id -Force;$wizard=$null;Pause 200
 RunSetup
 Assert (Test-Path $ExePath) 'Install missing app';Assert (Test-Path (Join-Path $install 'Uninstall.exe')) 'Uninstaller missing'
 $version=[Diagnostics.FileVersionInfo]::GetVersionInfo($ExePath);Assert ($version.ProductName -eq 'Keyisle' -and $version.ProductVersion -eq '1.0.0') 'Embedded product metadata incorrect'
 $icon=[Drawing.Icon]::ExtractAssociatedIcon($ExePath);$bitmap=$icon.ToBitmap();try{$pixel=$bitmap.GetPixel([int]($bitmap.Width/5),[int]($bitmap.Height/2));Assert ([Math]::Abs($pixel.R-17) -lt 12 -and [Math]::Abs($pixel.G-103) -lt 12 -and [Math]::Abs($pixel.B-95) -lt 12) 'Executable still uses generic icon'}finally{$bitmap.Dispose();$icon.Dispose()}
 $shell=New-Object -ComObject WScript.Shell
 foreach($path in @('test-shortcuts\menu\Keyisle.lnk','test-shortcuts\desktop\Keyisle.lnk')){$link=$shell.CreateShortcut((Join-Path $install $path));Assert ($link.TargetPath -eq $ExePath) "Broken shortcut: $path"}
 $entry=Get-ItemProperty -LiteralPath ($registry+'\Uninstall');Assert ($entry.DisplayVersion -eq '1.0.0') 'Uninstall version missing'
 Assert ($entry.UninstallString -eq ('"'+(Join-Path $install 'Uninstall.exe')+'"')) 'Uninstall command not quoted'
 Write-Output 'PASS: branded Unicode per-user installer creates app, multi-size icon/version resources, shortcuts and quoted uninstall registration in an isolated location.'
 StartApp;Unlock 'GUI-Master-Only!2026';SetEdit 0 'atrust';Assert (TextExists 'aTrust 测试') 'Legacy 0.12 account did not survive first-release upgrade';SetEdit 0 ''
 Button '新增账号';SetEdit 0 'Installer persisted account';SetEdit 1 'installer.user';SetEdit 2 'Installer-Fake!2026';Button '保存账号';StopApp
 Assert ($null -eq (Get-ItemProperty -LiteralPath $testRun -Name WindowsPasswordManager -ErrorAction SilentlyContinue).WindowsPasswordManager) 'Initial launch enabled a disabled startup entry'
 $before=Hash (Join-Path $vaultDir 'vault.db');$preferences=@{};foreach($name in @('lock-settings.txt','theme.txt','appearance.txt','shortcut.txt')){if(Test-Path (Join-Path $vaultDir $name)){$preferences[$name]=Hash (Join-Path $vaultDir $name)}}
 Set-Content -LiteralPath (Join-Path $install 'README.md') -Value 'old program file'
 RunSetup
 Assert ((Hash (Join-Path $vaultDir 'vault.db')) -eq $before) 'Cover install changed vault'
 foreach($name in $preferences.Keys){Assert ((Hash (Join-Path $vaultDir $name)) -eq $preferences[$name]) "Cover install changed preference: $name"}
 Assert ((Get-Content -Raw (Join-Path $install 'README.md')) -ne 'old program file') 'Cover install did not replace program docs'
 StartApp;RunSetup 10;StopApp;Assert ((Hash (Join-Path $vaultDir 'vault.db')) -eq $before) 'Busy install altered vault'
 Write-Output 'PASS: actual 0.12 fixture opens with preserved accounts; cover install preserves encrypted vault and preferences; running app blocks replacement without forced termination.'
 New-Item -ItemType Directory -Force $testRun|Out-Null
 Set-ItemProperty -LiteralPath $testRun -Name WindowsPasswordManager -Value '"C:\old-location\Keyisle.exe" --autostart'
 StartApp;Pause 300
 $command=(Get-ItemProperty -LiteralPath $testRun -Name WindowsPasswordManager).WindowsPasswordManager
 Assert ($command -eq ('"'+$ExePath+'" --autostart')) 'Enabled startup path did not migrate on first launch'
 StopApp
 $sentinel=Join-Path $install 'keep-me.db';[IO.File]::WriteAllText($sentinel,'synthetic user-added file')
 Uninstall
 Assert (Test-Path $sentinel) 'Uninstall recursively deleted user file'
 Assert ((Hash (Join-Path $vaultDir 'vault.db')) -eq $before) 'Uninstall removed or changed vault'
 Assert (-not (Test-Path ($registry+'\Uninstall'))) 'Uninstall registry remains'
 $remaining=(Get-ItemProperty -LiteralPath $testRun -Name WindowsPasswordManager -ErrorAction SilentlyContinue).WindowsPasswordManager
 Assert ($null -eq $remaining) 'Uninstall left own startup command'
 RunSetup;StartApp;Unlock 'GUI-Master-Only!2026';SetEdit 0 'Installer persisted account';Assert (TextExists 'Installer persisted account') 'Reinstall could not open retained account';StopApp;Uninstall
 Assert ((Get-ItemProperty -LiteralPath $realPath -Name WindowsPasswordManager -ErrorAction SilentlyContinue).WindowsPasswordManager -eq $realBefore) 'Test changed real startup registration'
 Assert ((Test-Path 'HKCU:\Software\Keyisle') -eq $realProductBefore) 'Test changed real product registration'
 Write-Output 'PASS: startup path relocates without enabling a disabled entry; uninstall removes own registration while preserving vault and unknown files; reinstall opens retained data; real product/Run keys stay unchanged.'
}finally{
 if($wizard -and -not $wizard.HasExited){Stop-Process -Id $wizard.Id -Force}
 StopApp
 Remove-Item -LiteralPath $registry -Recurse -Force -ErrorAction SilentlyContinue
 Remove-Item -LiteralPath $testRun -Force -ErrorAction SilentlyContinue
}
