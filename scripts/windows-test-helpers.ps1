$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$ownsTestRegistryKey=-not $env:PASSWORD_MANAGER_TEST_RUN_KEY
if($ownsTestRegistryKey){
 $env:PASSWORD_MANAGER_TEST_RUN_KEY='Software\PasswordHelperTests\GUI-'+[Guid]::NewGuid().ToString('N')
}
$testRegistryPath='Registry::HKEY_CURRENT_USER\'+$env:PASSWORD_MANAGER_TEST_RUN_KEY
if($ownsTestRegistryKey){Register-EngineEvent -SourceIdentifier PowerShell.Exiting -MessageData $testRegistryPath -Action {
 $path=$event.MessageData
 if($path -like 'Registry::HKEY_CURRENT_USER\Software\PasswordHelperTests\GUI-*'){Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue}
}|Out-Null}

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class VaultTestNative {
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h,out uint process);
 [DllImport("user32.dll")] public static extern void keybd_event(byte key,byte scan,uint flags,UIntPtr extra);
 [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h,uint command);
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
 [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
}
'@
function Pause([int]$ms = 300) { Start-Sleep -Milliseconds $ms }
function Assert($condition, [string]$message) { if (-not $condition) { throw $message } }
function Main {
 $condition = [Windows.Automation.AndCondition]::new(
  [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty,$script:process.Id),
  [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,'Keyisle · 账号管理'))
 $result = $null
 for($retry=0;$retry -lt 12 -and $null -eq $result;$retry++) {
  $result = [Windows.Automation.AutomationElement]::RootElement.FindFirst([Windows.Automation.TreeScope]::Children,$condition)
  if($null -eq $result){Pause 50}
 }
 Assert ($null -ne $result) 'Main window missing'
 return $result
}
function Visible($element, $property, $value) {
 $condition = [Windows.Automation.PropertyCondition]::new($property,$value)
 return @($element.FindAll([Windows.Automation.TreeScope]::Descendants,$condition) | Where-Object { -not $_.Current.IsOffscreen })
}
function Button([string]$name,[int]$delay=300) {
 $buttons = @(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Button) | Where-Object { $_.Current.Name -eq $name })
 if ($buttons.Count -eq 0 -and $name -in @('编辑账号','删除账号')) {
  $search=@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '搜索账号')
  Assert ($search.Count -gt 0) 'Account search missing when opening account menu'
  $search[0].SetFocus();[VaultTestNative]::SetForegroundWindow([IntPtr](Main).Current.NativeWindowHandle)|Out-Null
  [Windows.Forms.SendKeys]::SendWait('+{F10}');Pause 150
  $buttons=@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) $name)
 }
 if ($buttons.Count -eq 0 -and $name -in @('收起','退出')) {
  $more=@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '更多')
  if($more.Count -gt 0){
   $more[0].GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke();Pause 100
   $mapped=if($name -eq '收起'){'收起到托盘'}else{'退出程序'}
   $buttons=@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) $mapped)
  }
 }
 Assert ($buttons.Count -gt 0) "Button missing: $name"
 $buttons[0].GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
 Pause $delay
}
function Edits {
 $group = @(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '账号编辑表单')
 if ($group.Count -gt 0) {
  $condition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty,[Windows.Automation.ControlType]::Edit)
  return @($group[0].FindAll([Windows.Automation.TreeScope]::Descendants,$condition))
 }
 return @(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Edit))
}
function SetEdit([int]$index, [string]$text) {
 $edits = @(Edits)
 Assert ($edits.Count -gt $index) "Missing edit index $index, count=$($edits.Count)"
 $edits[$index].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue($text)
 Pause 80
}
function TextExists([string]$text) {
 return @(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) $text).Count -gt 0
}
function Unlock([string]$password = 'GUI-Master-Only!2026') {
 SetEdit 0 $password
 Button '解锁'
 Pause 1500
}
function StartApp([switch]$NoFocus) {
 $script:process = Start-Process -FilePath $ExePath -PassThru -RedirectStandardError ($DataDir + '\stderr.log') -RedirectStandardOutput ($DataDir + '\stdout.log')
 $null = $script:process.Handle
 for ($i=0; $i -lt 80; $i++) {
  Pause 200
  Assert (-not $script:process.HasExited) 'Exited at startup'
  try { $null = Main; break } catch { if ($i -eq 79) { throw } }
 }
 Pause 300
 if ($NoFocus) {return}
 $edits = @(Edits)
 if ($edits.Count -gt 0) {
  $edits[0].SetFocus()
  $handle = [IntPtr](Main).Current.NativeWindowHandle
  [VaultTestNative]::SetForegroundWindow($handle) | Out-Null
  Pause 100
  Assert ([VaultTestNative]::GetForegroundWindow() -eq $handle) 'App lost foreground during test initialization'
  [Windows.Forms.SendKeys]::SendWait('^a')
 }
}
function StopApp {
 foreach ($title in @('保存加密备份（请选择新文件）','选择要恢复的加密备份')) {
  $dialog = [VaultTestNative]::FindWindow('#32770',$title)
  if ($dialog -ne [IntPtr]::Zero -and [Windows.Automation.AutomationElement]::FromHandle($dialog).Current.ProcessId -eq $script:process.Id) { [VaultTestNative]::PostMessage($dialog,0x10,[IntPtr]::Zero,[IntPtr]::Zero) | Out-Null }
 }
 Pause 200
 $h = [VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents')
 if ($h -ne [IntPtr]::Zero -and $script:process -and -not $script:process.HasExited) {
  [uint32]$owner=0;[VaultTestNative]::GetWindowThreadProcessId($h,[ref]$owner)|Out-Null
  if($owner -eq $script:process.Id){
   [VaultTestNative]::PostMessage($h,0x10,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null
   Pause 250
   try {if(TextExists '未保存的修改'){Button '放弃修改并退出'}}catch{}
  }
 }
 if ($script:process -and -not $script:process.HasExited) { if (-not $script:process.WaitForExit(6000)) { Write-Warning 'App did not exit within 6 seconds' } }
 Pause 300
}
function SessionLock {
 $h = [VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents')
 Assert ($h -ne [IntPtr]::Zero) 'Native event window missing'
 [VaultTestNative]::PostMessage($h,0x2B1,[IntPtr]7,[IntPtr]::Zero) | Out-Null
 Pause 500
}
function DialogFile([string]$title,[string]$path) {
 $dialog = [IntPtr]::Zero
 for ($i=0; $i -lt 40; $i++) { $dialog = [VaultTestNative]::FindWindow('#32770',$title); if ($dialog -ne [IntPtr]::Zero) { break }; Pause 100 }
 Assert ($dialog -ne [IntPtr]::Zero) "File dialog missing: $title"
 $root = [Windows.Automation.AutomationElement]::FromHandle($dialog)
 Pause 600
 $condition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty,[Windows.Automation.ControlType]::Edit)
 $edit = $null
 for ($i=0; $i -lt 60; $i++) {
  $edits = @($root.FindAll([Windows.Automation.TreeScope]::Descendants,$condition) | Where-Object { -not $_.Current.IsOffscreen })
  $matches = @($edits | Where-Object { $_.Current.Name -match '文件名|File name' -or $_.Current.AutomationId -eq '1001' })
  if ($matches.Count -gt 0) { $edit = $matches[0]; break }
  Pause 100
 }
 Assert ($null -ne $edit) 'File name edit missing'
 $edit.SetFocus()
 Assert ([VaultTestNative]::GetForegroundWindow() -eq $dialog) 'File dialog lost foreground'
 $edit.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue($path)
 Write-Output ('DIALOG FILE: ' + $edit.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value)
 [Windows.Forms.SendKeys]::SendWait('{ENTER}')
 for ($i=0; $i -lt 60; $i++) {
  Pause 100
  if ([VaultTestNative]::FindWindow('#32770',$title) -eq [IntPtr]::Zero) { break }
 }
 Assert ([VaultTestNative]::FindWindow('#32770',$title) -eq [IntPtr]::Zero) 'File dialog did not close'

 Pause 600
}

# SendKeys can reactivate its saved foreground after Enter; use actual key events for fill commands.
function SendTestKeys([string]$keys){
 $modifier=switch($keys){'{ENTER}'{0};'^{ENTER}'{0x11};'+{ENTER}'{0x10};default{-1}}
 if($modifier -lt 0){[Windows.Forms.SendKeys]::SendWait($keys);return}
 if($modifier -ne 0){[VaultTestNative]::keybd_event([byte]$modifier,0,0,[UIntPtr]::Zero)}
 [VaultTestNative]::keybd_event(0x0d,0,0,[UIntPtr]::Zero);[VaultTestNative]::keybd_event(0x0d,0,2,[UIntPtr]::Zero)
 if($modifier -ne 0){[VaultTestNative]::keybd_event([byte]$modifier,0,2,[UIntPtr]::Zero)}
}
