param([string]$ExePath,[string]$DataDir)
. (Join-Path $PSScriptRoot 'windows-test-helpers.ps1')
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;using System.Runtime.InteropServices;
public static class ResetNative {
 [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
 [DllImport("user32.dll")] public static extern void mouse_event(uint flags,uint x,uint y,uint data,UIntPtr extra);
}
'@
function Pause([int]$ms=300){$watch=[Diagnostics.Stopwatch]::StartNew();while($watch.ElapsedMilliseconds -lt $ms){[Windows.Forms.Application]::DoEvents();Start-Sleep -Milliseconds 10}}
function Named([string]$name,$type=[Windows.Automation.ControlType]::Button){
 [VaultTestNative]::SetForegroundWindow([IntPtr](Main).Current.NativeWindowHandle)|Out-Null;Pause 80
 $match=[Windows.Automation.AndCondition]::new([Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,$name),[Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty,$type))
 $item=(Main).FindFirst([Windows.Automation.TreeScope]::Descendants,$match)
 if($null -eq $item -and $name -eq '一键还原'){
  $r=(Main).Current.BoundingRectangle;[ResetNative]::SetCursorPos([int]($r.Right-36),[int]($r.Top+$r.Height/2))|Out-Null
  [ResetNative]::mouse_event(0x800,0,0,4294964896,[UIntPtr]::Zero);Pause 350
  $item=(Main).FindFirst([Windows.Automation.TreeScope]::Descendants,$match)
 }
 if($null -eq $item){
  $anchor=@(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Button) | Where-Object {$_.Current.IsEnabled -and $_.Current.Name -in @('保存设置','返回账号列表')});if($anchor.Count -gt 0){$anchor[0].SetFocus()}else{(Edits)[0].SetFocus()}
  for($i=0;$i -lt 40;$i++){[Windows.Forms.SendKeys]::SendWait('{TAB}');Pause 60;$item=(Main).FindFirst([Windows.Automation.TreeScope]::Descendants,$match);if($item){break}}
 }
 Assert ($null -ne $item) "Control missing: $name"
 $item.SetFocus();Pause 120;return $item
}
function InvokeNamed([string]$name){(Named $name).GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke();Pause 300}
function Checkbox([string]$name){return Named $name ([Windows.Automation.ControlType]::CheckBox)}
function Toggle([string]$name){(Checkbox $name).GetCurrentPattern([Windows.Automation.TogglePattern]::Pattern).Toggle();Pause 100}
function Combo([string]$name,[int]$index){$item=Named $name ([Windows.Automation.ControlType]::ComboBox);[Windows.Forms.SendKeys]::SendWait('{UP}{UP}{UP}{UP}{UP}');for($i=0;$i -lt $index;$i++){[Windows.Forms.SendKeys]::SendWait('{DOWN}')};Pause 150}
function SetResetMaster([string]$value){$item=Named '还原验证主密码' ([Windows.Automation.ControlType]::Edit);$item.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue($value);Pause 100}
function WaitResetIdle{
 for($i=0;$i -lt 100;$i++){Pause 100;if(-not (TextExists '正在验证并还原；完成后软件将重新启动…')){return}}
 throw 'Reset job did not finish'
}
function Capture([string]$name){$r=(Main).Current.BoundingRectangle;$b=[Drawing.Bitmap]::new([int]$r.Width,[int]$r.Height);$g=[Drawing.Graphics]::FromImage($b);try{$g.CopyFromScreen([int]$r.X,[int]$r.Y,0,0,$b.Size);$b.Save((Join-Path $DataDir $name),[Drawing.Imaging.ImageFormat]::Png)}finally{$g.Dispose();$b.Dispose()}}
function SharedHash([string]$path){
 $stream=[IO.File]::Open($path,[IO.FileMode]::Open,[IO.FileAccess]::Read,([IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete));$sha=[Security.Cryptography.SHA256]::Create()
 try{return [BitConverter]::ToString($sha.ComputeHash($stream))}finally{$sha.Dispose();$stream.Dispose()}
}
function TakeSnapshot{
 $snapshot=@{}
 foreach($path in Get-ChildItem -LiteralPath $DataDir -File -Recurse){if($path.Extension -eq '.db' -or $path.Extension -eq '.txt'){$snapshot[$path.FullName]=(SharedHash $path.FullName)}}
 return $snapshot
}
function AssertSnapshot($snapshot){foreach($path in $snapshot.Keys){Assert (Test-Path -LiteralPath $path) "Failed authentication removed file: $path";Assert ((SharedHash $path) -eq $snapshot[$path]) "Failed authentication changed file: $path"}}
function Registration{try{return (Get-ItemProperty -LiteralPath $testRegistryPath -Name WindowsPasswordManager -ErrorAction Stop).WindowsPasswordManager}catch{return $null}}
function AdoptRestart([int]$oldId){
 for($i=0;$i -lt 120;$i++){
  Pause 100;$events=[VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents');[uint32]$owner=0
  if($events -ne [IntPtr]::Zero){[VaultTestNative]::GetWindowThreadProcessId($events,[ref]$owner)|Out-Null}
  if($owner -ne 0 -and $owner -ne $oldId){$script:process=Get-Process -Id $owner;$null=$script:process.Handle;try{$null=Main;return}catch{}}
 }
 throw 'Completed reset did not restart into a fresh process'
}
Assert ($DataDir -match 'test-v1\.0\.0-') 'Reset tests require an explicitly isolated v1.0 directory'
Assert (-not (Test-Path (Join-Path $DataDir 'vault.db'))) 'Use an empty isolated profile'
Assert ([VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents') -eq [IntPtr]::Zero) 'Close existing app before testing'
$previous=$env:PASSWORD_MANAGER_DATA_DIR;$env:PASSWORD_MANAGER_DATA_DIR=$DataDir;New-Item -ItemType Directory -Force $DataDir|Out-Null
$script:process=$null;$blocker=$null;$external=$DataDir+'-external.db'
$realRunPath='HKCU:\Software\Microsoft\Windows\CurrentVersion\Run';$realRunBefore=(Get-ItemProperty -LiteralPath $realRunPath -Name WindowsPasswordManager -ErrorAction SilentlyContinue).WindowsPasswordManager
trap {Write-Output $_.Exception.Message;Write-Output $_.ScriptStackTrace;try{Capture 'failure.png'}catch{};throw}
try{
 StartApp;SetEdit 0 'GUI-Master-Only!2026';SetEdit 1 'GUI-Master-Only!2026';Button '创建密码库';Pause 1500
 Button '新增账号';SetEdit 0 'Reset private account';SetEdit 1 'reset.user';SetEdit 2 'Reset-Fake-Only!2026';Button '保存账号'
 Button '编辑账号';SetEdit 0 'Reset private account edited';Button '保存账号'
 Button '备份管理';Button '加密备份';DialogFile '保存加密备份（请选择新文件）' $external
 Button '恢复备份';DialogFile '选择要恢复的加密备份' $external;Unlock
 $active=[IO.File]::ReadAllText((Join-Path $DataDir 'last-vault.txt'));Assert ($active -like '*restored-*.db') 'Fixture did not enter a restored vault'
 Copy-Item -LiteralPath $active -Destination (Join-Path $DataDir 'vault.pre-v04-123.db')
 Button '设置';Combo '外观模式' 2;Combo '外观配色' 2;Combo '自动锁定时间' 2;Combo '空闲计时依据' 1
 Toggle '开机启动（当前 Windows 用户）';Toggle '开机启动时留在托盘';Button '保存设置';Pause 700
 Assert ($null -ne (Registration)) 'Fixture startup registration not enabled in isolated key'
 $snapshot=TakeSnapshot;$registration=Registration;$exportHash=(Get-FileHash -LiteralPath $external -Algorithm SHA256).Hash
 InvokeNamed '录制快捷键';for($i=0;$i -lt 40 -and -not (TextExists '请按快捷键组合；Esc 取消。已占用的组合也可录入。');$i++){Pause 50};InvokeNamed '一键还原';Capture '01-reset-confirm.png'
 Assert (@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '取消录制').Count -eq 0) 'Reset dialog left shortcut recording active'
 $submit=@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '验证主密码并还原')[0]
 Assert (-not $submit.Current.IsEnabled) 'Reset allowed empty password and missing explicit confirmation'
 SetResetMaster 'wrong-reset-password';Toggle '我确认永久清除上述数据并恢复默认设置'
 (Named '还原验证主密码' ([Windows.Automation.ControlType]::Edit))|Out-Null
 [Windows.Forms.SendKeys]::SendWait('{ENTER}');Pause 100;Assert (-not (Test-Path (Join-Path $DataDir 'reset-pending'))) 'Enter in password field committed reset'
 Button '验证主密码并还原' 0;WaitResetIdle;Capture '02-reset-wrong-master.png'
 AssertSnapshot $snapshot;Assert (-not (Test-Path (Join-Path $DataDir 'reset-pending'))) 'Wrong master created a committed reset'
 Assert ((Registration) -eq $registration) 'Wrong master changed startup registration'
 Button '取消还原';Assert (TextExists '解锁密码库') 'Cancelled reset did not return to a locked vault';Unlock
 Assert (TextExists 'Reset private account edited') 'Wrong master or cancel lost account data'
 Write-Output 'PASS: reset requires current master and explicit confirmation; wrong master/cancellation leave vaults, backups, settings and isolated startup registration intact.'
 Button '设置';InvokeNamed '一键还原';SetResetMaster 'GUI-Master-Only!2026';Toggle '我确认永久清除上述数据并恢复默认设置'
 # Shift+Tab wraps from the first field to the final action; Tab wraps back.
 (Named '还原验证主密码' ([Windows.Automation.ControlType]::Edit))|Out-Null
 [Windows.Forms.SendKeys]::SendWait('+{TAB}');Pause 100
 Assert ([Windows.Automation.AutomationElement]::FocusedElement.Current.Name -eq '验证主密码并还原') 'Reset dialog did not trap reverse Tab'
 [Windows.Forms.SendKeys]::SendWait('{TAB}');Pause 100
 Assert ([Windows.Automation.AutomationElement]::FocusedElement.Current.Name -eq '还原验证主密码') 'Reset dialog did not trap forward Tab'
 $oldId=$script:process.Id;Button '验证主密码并还原' 0;AdoptRestart $oldId;Pause 700;Capture '03-reset-created-empty.png'
 Assert (TextExists '创建本地密码库') 'Completed reset did not show create-vault page'
 foreach($name in @('vault.db','theme.txt','appearance.txt','shortcut.txt','launch-settings.txt','lock-settings.txt','window.txt','last-vault.txt','reset-pending')){Assert (-not (Test-Path (Join-Path $DataDir $name))) "Reset retained owned data: $name"}
 Assert (@(Get-ChildItem -LiteralPath $DataDir -Filter 'restored-*.db').Count -eq 0) 'Reset retained restored vaults'
 Assert (@(Get-ChildItem -LiteralPath $DataDir -Filter '*.pre-v04-*.db').Count -eq 0) 'Reset retained upgrade backups'
 Assert (-not (Test-Path (Join-Path $DataDir 'backups'))) 'Reset retained managed automatic backups'
 Assert ($null -eq (Registration)) 'Reset retained isolated logon startup'
 Assert ((Get-FileHash -LiteralPath $external -Algorithm SHA256).Hash -eq $exportHash) 'Reset altered external manual backup'
 SetEdit 0 'GUI-New-Master!2026';SetEdit 1 'GUI-New-Master!2026';Button '创建密码库';Pause 1400
 Assert (TextExists '0 个账号') 'New vault after reset contains old records'
 Assert (TextExists '电脑空闲 5 分钟自动锁定 · 不自动提交登录') 'Reset did not restore default idle policy'
 Button '锁定';SetEdit 0 'GUI-Master-Only!2026';Button '解锁';Pause 1000;Assert (TextExists '解锁密码库') 'Old master opened newly-created vault';Unlock 'GUI-New-Master!2026'
 Write-Output 'PASS: authenticated reset removes current/original/restored vaults, managed backups and all preference files; restarts to defaults; external backup survives and old master cannot open the new vault.'
 Button '设置';InvokeNamed '一键还原';SetResetMaster 'GUI-New-Master!2026';Toggle '我确认永久清除上述数据并恢复默认设置'
 $active=[IO.File]::ReadAllText((Join-Path $DataDir 'last-vault.txt'))
 $blocker=[IO.File]::Open($active,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::ReadWrite)
 Button '验证主密码并还原' 0;WaitResetIdle;Capture '04-reset-resumable.png'
 Assert (Test-Path (Join-Path $DataDir 'reset-pending')) 'File-in-use reset lost resume marker'
 Assert (TextExists '继续完成还原') 'File-in-use reset falsely reported success'
 Assert (-not @(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '取消还原')[0].Current.IsEnabled) 'Committed reset could be cancelled into partial data'
 StopApp;$blocker.Dispose();$blocker=$null;StartApp;Pause 700
 Assert (TextExists '创建本地密码库') 'Cold recovery did not finish previously-authorized deletion'
 Assert (-not (Test-Path (Join-Path $DataDir 'vault.db')) -and -not (Test-Path (Join-Path $DataDir 'reset-pending'))) 'Cold recovery left old vault or marker'
 Assert ((Get-FileHash -LiteralPath $external -Algorithm SHA256).Hash -eq $exportHash) 'Resumed reset altered external backup'
 Capture '05-reset-recovery.png'
 $realRunAfter=(Get-ItemProperty -LiteralPath $realRunPath -Name WindowsPasswordManager -ErrorAction SilentlyContinue).WindowsPasswordManager
 Assert ($realRunAfter -eq $realRunBefore) 'Reset test modified the real user Run entry'
 Write-Output 'PASS: Windows file-in-use leaves an explicit committed recovery state; after releasing the file, cold startup finishes deletion; real Run registration remains unchanged.'
 }finally{
 if($blocker){$blocker.Dispose()}
 if($script:process -and $script:process.HasExited){
  $events=[VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents');[uint32]$owner=0
  if($events -ne [IntPtr]::Zero){[VaultTestNative]::GetWindowThreadProcessId($events,[ref]$owner)|Out-Null}
  if($owner -ne 0){$child=Get-CimInstance Win32_Process -Filter "ProcessId = $owner";if($child.ParentProcessId -eq $script:process.Id){$script:process=Get-Process -Id $owner}}
 }
 StopApp;$env:PASSWORD_MANAGER_DATA_DIR=$previous
}
