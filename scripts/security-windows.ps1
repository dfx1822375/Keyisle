param([Parameter(Mandatory=$true)][string]$ExePath,[Parameter(Mandatory=$true)][string]$DataDir,[switch]$TestIdle,[switch]$TestHelloFallback,[switch]$TestSystemTimeout)
. (Join-Path $PSScriptRoot 'windows-test-helpers.ps1')
trap { Write-Output $_.Exception.Message; Write-Output $_.ScriptStackTrace; throw }
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class OwnedCredentials {
 public delegate bool Visit(IntPtr h,IntPtr p);
 [DllImport("user32.dll")] static extern bool EnumWindows(Visit callback,IntPtr p);
 [DllImport("user32.dll")] static extern IntPtr GetWindow(IntPtr h,uint kind);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr h,StringBuilder text,int count);
 public static IntPtr Find(IntPtr owner){
  IntPtr result=IntPtr.Zero;
  EnumWindows((h,state)=>{var c=new StringBuilder(80);GetClassName(h,c,80);if(c.ToString()=="Credential Dialog Xaml Host"){var p=GetWindow(h,4);for(int depth=0;depth<8 && p!=IntPtr.Zero;depth++){if(p==owner){result=h;return false;}p=GetWindow(p,4);}}return true;},IntPtr.Zero);
  return result;
 }
}
'@
function SensitiveCheckbox {
 $condition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,'敏感项：使用、查看和编辑前再次验证')
 $item = (Main).FindFirst([Windows.Automation.TreeScope]::Descendants,$condition)
 Assert ($null -ne $item) 'Sensitive checkbox missing'
 return $item
}
function SetSensitive([bool]$value) {
 $item = SensitiveCheckbox
 $toggle = $item.GetCurrentPattern([Windows.Automation.TogglePattern]::Pattern)
 if (($toggle.Current.ToggleState -eq [Windows.Automation.ToggleState]::On) -ne $value) { $toggle.Toggle(); Pause 150 }
}
function Verify([string]$password='GUI-Master-Only!2026') {
 Assert (TextExists '敏感项验证') 'Sensitive operation bypassed verification'
 Button '密码库主密码'
 $items = @(Edits); $items[0].SetFocus(); Pause 150
 SetEdit 0 $password
 Button '验证主密码'
 Pause 1500
}
function SelectCombo([int]$index,[int]$steps) {
 $name=@('自动锁定时间','空闲计时依据')[$index]
 $match=[Windows.Automation.AndCondition]::new([Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,$name),[Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty,[Windows.Automation.ControlType]::ComboBox))
 $item=(Main).FindFirst([Windows.Automation.TreeScope]::Descendants,$match)
 if($null -eq $item){
  @(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '保存设置')[0].SetFocus()
  [VaultTestNative]::SetForegroundWindow([IntPtr](Main).Current.NativeWindowHandle)|Out-Null
  for($i=0;$i -lt 40;$i++){[Windows.Forms.SendKeys]::SendWait('{TAB}');Pause 80;$item=(Main).FindFirst([Windows.Automation.TreeScope]::Descendants,$match);if($item){break}}
 }
 Assert ($null -ne $item) "Settings combo missing: $name"
 $item.SetFocus()
 $handle = [IntPtr](Main).Current.NativeWindowHandle
 [VaultTestNative]::SetForegroundWindow($handle) | Out-Null
 Pause 150
 Assert ([VaultTestNative]::GetForegroundWindow() -eq $handle) 'Settings lost foreground'
 for ($i=0; $i -lt 8; $i++) { [Windows.Forms.SendKeys]::SendWait('{UP}') }
 for ($i=0; $i -lt $steps; $i++) { [Windows.Forms.SendKeys]::SendWait('{DOWN}') }
 Pause 300
}
Assert (-not (Test-Path (Join-Path $DataDir 'vault.db'))) 'Security test requires an empty isolated profile'
Assert ([VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents') -eq [IntPtr]::Zero) 'Close existing app first'
$previous = $env:PASSWORD_MANAGER_DATA_DIR
$env:PASSWORD_MANAGER_DATA_DIR = $DataDir
New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
$script:process = $null
try {
 StartApp
 SetEdit 0 'GUI-Master-Only!2026'; SetEdit 1 'GUI-Master-Only!2026'; Button '创建密码库'; Pause 1600
 Button '新增账号'
 SetEdit 0 'Protected test'; SetEdit 1 'sensitive.user'; SetEdit 2 'Sensitive-Test!2026'; SetEdit 3 'Security'; SetEdit 4 'sensitive,test'; SetSensitive $true
 Button '保存账号'
 Assert (TextExists '敏感 · Protected test') 'Sensitive marker not shown'
 Assert (-not (TextExists 'sensitive.user')) 'Sensitive username exposed in list'
 Button '编辑账号'; Verify 'wrong-password'
 Assert (TextExists '敏感项验证') 'Wrong re-verification password accepted'
 Button '取消验证'
 Assert (-not (TextExists '账号信息')) 'Cancel opened sensitive editor'
 Write-Output 'PASS: sensitive list hides username; wrong master and cancellation cannot open editor.'
 Button '编辑账号'
 Assert (@(Edits).Count -eq 0) 'Application is still collecting Windows password'
 $owner=[IntPtr](Main).Current.NativeWindowHandle
 Button '打开 Windows 验证'
 $system=$null
 for($i=0;$i -lt 60;$i++) {
  $handle=[OwnedCredentials]::Find($owner)
  if($handle -ne [IntPtr]::Zero){$system=[Windows.Automation.AutomationElement]::FromHandle($handle);break};Pause 100
 }
 if($null -eq $system){@(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Text)) | ForEach-Object {Write-Output $_.Current.Name}}
 Assert ($null -ne $system) 'System credential window did not open'
 Pause 1000
 Button '取消验证';Pause 700
 Assert (-not (TextExists '账号信息')) 'Cancellation opened protected editor'
 Button '编辑账号'
 Button '打开 Windows 验证';Pause 1200;SessionLock;Pause 1800
 Assert (TextExists '解锁密码库') 'System dialog blocked vault lock'
 Unlock
 if($TestSystemTimeout) {
  Button '编辑账号';Button '打开 Windows 验证'
  Write-Output 'STAGE: waiting for native verification timeout without entering credentials.'
  Pause 62500
  if(-not (TextExists '敏感项验证')){@(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Text)) | ForEach-Object {Write-Output ('TIMEOUT STATE: '+$_.Current.Name)}}
  Assert (-not (TextExists '账号信息')) 'Timed-out request exposed editor'
  $helpers=@(Get-CimInstance Win32_Process -Filter ('ParentProcessId='+$script:process.Id) | Where-Object { $_.Name -eq 'windows-password-manager.exe' -and $_.CommandLine -like '*--windows-auth-helper*' })
  Assert ($helpers.Count -eq 0) 'Expired native verifier process survived'
  if(TextExists '解锁密码库'){Unlock}elseif(TextExists '敏感项验证'){Button '取消验证'}
 }
 Button '编辑账号';Button '取消验证' 
 Write-Output 'PASS: Windows credentials are collected in system popup; app cancellation and session lock close verifier without authorization.'
 if ($TestHelloFallback) {
  Button '编辑账号'; Button 'PIN / Hello'; Button 'Windows PIN / Hello'; Pause 1500
  Assert (TextExists '敏感项验证') 'Unavailable Hello unexpectedly authorized action'
  Assert (@(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Text) | Where-Object { $_.Current.Name -match '未执行操作|无法打开 Windows PIN|不支持此验证接口' }).Count -gt 0) 'System verification did not show a safe master fallback'
  @(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Text) | Where-Object { $_.Current.Name -match '未执行操作|无法打开 Windows PIN|不支持此验证接口' }) | ForEach-Object { Write-Output ('SYSTEM VERIFICATION: '+$_.Current.Name) }
  Assert (-not (TextExists '账号信息')) 'Unavailable Hello exposed editor'
  Verify
  Assert (TextExists '账号信息') 'Master fallback after unavailable Hello failed'
  Button '取消编辑'
  Write-Output 'PASS: real unavailable Windows Hello cannot authorize; master-password fallback succeeds.'
 }

 Button '编辑账号'; Verify
 Assert (TextExists '账号信息') 'Correct master did not authorize editor'
 $edits = @(Edits)
 Assert ($edits[1].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -eq 'sensitive.user') 'Protected username changed'
 Button '取消编辑'
 Button '编辑账号'
 Assert (TextExists '敏感项验证') 'Prior approval was reused for another edit'
 Button '密码库主密码'; SetEdit 0 'GUI-Master-Only!2026'; Button '验证主密码' 0; SessionLock; Pause 1500
 Assert (TextExists '解锁密码库') 'Verification completion reopened a locked vault'
 Unlock
 Button '编辑账号'; Verify
 SetSensitive $false; Button '保存账号'
 Assert (TextExists 'Protected test') 'Sensitive removal failed after verified edit'
 Button '编辑账号'
 Assert (TextExists '账号信息') 'Normal record still requests sensitive authentication'
 SetSensitive $true; Button '保存账号'
 Write-Output 'PASS: approvals are single-use; lock invalidates in-flight verification; removing marker requires verified editing.'
 Button '删除账号'; Button '确认删除'
 Assert (TextExists '敏感项验证') 'Sensitive deletion bypassed verification'
 Button '取消验证'
 Assert (TextExists '1 个账号') 'Cancelled protected deletion changed vault'
 StopApp
 StartApp; Unlock
 Assert (TextExists '敏感 · Protected test') 'Sensitive flag lost after restart'
 $backup = Join-Path $DataDir 'protected-backup.db'
 Button '备份管理';Button '加密备份'; DialogFile '保存加密备份（请选择新文件）' $backup
 Button '恢复备份'; DialogFile '选择要恢复的加密备份' $backup
 Unlock
 Assert (TextExists '敏感 · Protected test') 'Backup recovery lost sensitive flag'
 Button '编辑账号'
 Assert (TextExists '敏感项验证') 'Recovered sensitive item bypassed verification'
 Button '取消验证'
 Write-Output 'PASS: encrypted backup and recovered working copy preserve sensitive verification.'
 Button '设置'
 SelectCombo 0 2
 SelectCombo 1 1
 Button '保存设置'; Button '返回账号列表'
 $settings = [IO.File]::ReadAllLines((Join-Path $DataDir 'lock-settings.txt'))
 Assert ($settings[0] -eq '15' -and $settings[1] -eq 'vault') 'Settings were not saved correctly'
 StopApp
 StartApp; Unlock
 Assert (TextExists '密码库空闲 15 分钟锁定') 'Settings did not survive restart'
 Write-Output 'PASS: sensitive deletion cannot bypass verification; flag and configurable timeout survive restart.'
 if ($TestIdle) {
  Button '设置'; SelectCombo 0 0; SelectCombo 1 1; Button '保存设置'; Button '返回账号列表'
  Assert (TextExists '密码库空闲 1 分钟锁定') 'One-minute setting not applied'
  Write-Output 'STAGE: waiting for one-minute vault inactivity while a separate app receives input.'
  $receiver = [Windows.Forms.Form]::new()
  $receiver.Text = 'Password Manager - separate idle-test receiver'
  $input = [Windows.Forms.TextBox]::new()
  $receiver.Controls.Add($input)
  try {
   $receiver.Show(); $receiver.Activate(); $input.Focus() | Out-Null
   [VaultTestNative]::SetForegroundWindow($receiver.Handle) | Out-Null
   for ($i=0; $i -lt 72; $i++) {
    Pause 1000
    [Windows.Forms.Application]::DoEvents()
    if ($i % 10 -eq 0 -and [VaultTestNative]::GetForegroundWindow() -eq $receiver.Handle) { [Windows.Forms.SendKeys]::SendWait('x') }
    if (TextExists '解锁密码库') { break }
   }
  } finally { $receiver.Dispose() }
  Assert (TextExists '解锁密码库') 'Actual configured vault inactivity timeout did not lock'
  Unlock
  Write-Output 'PASS: actual configured one-minute vault inactivity locks the vault.'
 }
 Button '删除账号'; Button '确认删除'; Verify
 Assert (TextExists '0 个账号') 'Verified protected deletion failed'
 Write-Output 'PASS: verified sensitive deletion succeeds.'
} finally {
 StopApp
 $env:PASSWORD_MANAGER_DATA_DIR = $previous
}
