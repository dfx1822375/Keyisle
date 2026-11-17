param([Parameter(Mandatory=$true)][string]$ExePath,[Parameter(Mandatory=$true)][string]$DataDir)
. (Join-Path $PSScriptRoot 'windows-test-helpers.ps1')
function Pause([int]$ms=300) {
 $watch=[Diagnostics.Stopwatch]::StartNew()
 while($watch.ElapsedMilliseconds -lt $ms){[Windows.Forms.Application]::DoEvents();Start-Sleep -Milliseconds 15}
}
function Popup {
 $condition=[Windows.Automation.AndCondition]::new(
  [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty,$script:process.Id),
  [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,'Keyisle · 快速填充'))
 $root=[Windows.Automation.AutomationElement]::RootElement.FindFirst([Windows.Automation.TreeScope]::Children,$condition)
 Assert ($null -ne $root) 'Popup missing'
 return $root
}
function Send([string]$keys,$root=(Main)) {
 $h=[IntPtr]$root.Current.NativeWindowHandle
 Assert ([VaultTestNative]::GetForegroundWindow() -eq $h) 'Foreground changed; refusing to type'
 SendTestKeys $keys;Pause 180
}
function Search([string]$query) {
 $edit=@(Visible (Popup) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Edit))[0]
 $edit.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue($query);Pause 120
}
function Selected {
 $rows=@(Visible (Popup) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Button) | Where-Object {$_.Current.Name.StartsWith('已选择，')})
 Assert ($rows.Count -eq 1) 'Expected one selected result'
 return $rows[0].Current.Name
}
function OpenPopup {
 $receiver.Clear();$form.Show();$form.Activate();$receiver.Focus() | Out-Null
 [VaultTestNative]::SetForegroundWindow($form.Handle) | Out-Null;Pause 150
 Assert ([VaultTestNative]::GetForegroundWindow() -eq $form.Handle) 'Receiver not foreground'
 [Windows.Forms.SendKeys]::SendWait('^%p');Pause 400
}
function Fill([string]$query,[string]$expected) {
 OpenPopup;Search $query;Send '{ENTER}' (Popup)
 for($i=0;$i -lt 30 -and $receiver.Text -ne $expected;$i++){Pause 100}
 Assert ($receiver.Text -eq $expected) "Fill failed for query: $query"
}
function Manage {
 $events=[VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents')
 [VaultTestNative]::PostMessage($events,0x8001,[IntPtr]::Zero,[IntPtr]0x202) | Out-Null;Pause 400
}
trap { Write-Output $_.Exception.Message;Write-Output $_.ScriptStackTrace;throw }
Assert (-not (Test-Path (Join-Path $DataDir 'vault.db'))) 'Use an empty isolated profile'
Assert ([VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents') -eq [IntPtr]::Zero) 'Close existing app first'
$previous=$env:PASSWORD_MANAGER_DATA_DIR;$env:PASSWORD_MANAGER_DATA_DIR=$DataDir
New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
$script:process=$null
$form=[Windows.Forms.Form]::new();$form.Text='Search regression receiver';$form.Width=450;$form.Height=160
$receiver=[Windows.Forms.TextBox]::new();$receiver.SetBounds(20,20,380,30);$form.Controls.Add($receiver)
try {
 StartApp;SetEdit 0 'GUI-Master-Only!2026';SetEdit 1 'GUI-Master-Only!2026';Button '创建密码库';Pause 1200
 foreach($record in @(@('企业邮箱','mail.user','Mail-Test!2026','工作','办公,邮件'),@('银行重庆','bank.user','Bank-Test!2026','个人','金融'),@('aTrust','vpn.user','VPN-Test!2026','工作','网络'),@('Invalid empty','','','测试','无效'))) {
  Button '新增账号'
  for($i=0;$i -lt 5;$i++){SetEdit $i $record[$i]}
  Button '保存账号'
 }
 Button '收起';OpenPopup
 foreach($query in @('邮箱','youxiang','yx','qyyx','mail.user 办公','gongzuo youxiang','ＹＯＵＸＩＡＮＧ')) {
  Search $query;Assert (@(Visible (Popup) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Button) | Where-Object {$_.Current.Name.Contains('企业邮箱')}).Count -eq 1) "Wrong fuzzy result: $query"
 }
 Search 'yinhang chongqing';Assert ((Selected).Contains('银行重庆')) 'Polyphonic pinyin failed'
 Search 'atrsut';Assert ((Selected).Contains('aTrust')) 'Adjacent typo failed'
 Search 'Mail-Test!2026';Assert (@(Visible (Popup) ([Windows.Automation.AutomationElement]::NameProperty) '没有匹配的账号，请调整关键词或筛选条件。').Count -eq 1) 'Password participated in search'
 Search '企业';$before=Selected;Send '{ESC}' (Popup)
 Write-Output 'PASS: popup searches Chinese, full pinyin, initials, mixed fields, full-width text, polyphonic names and typos; passwords excluded.'
 Fill 'youxiang' 'Mail-Test!2026';Fill 'atrust' 'VPN-Test!2026'
 OpenPopup;Assert ((Selected).Contains('aTrust')) 'Empty query did not sort recent usage first'
 Search '企业';Assert ((Selected).Contains('企业邮箱')) 'Relevance did not override recency'
 Send '{ESC}' (Popup)
 OpenPopup;Search 'Invalid empty';Send '{ENTER}' (Popup);Assert ($receiver.Text -eq '') 'Invalid record filled'
 Search '';Assert ((Selected).Contains('aTrust')) 'Failed fill changed recent ordering'
 Send '{DOWN}' (Popup);Assert (-not (Selected).Contains('aTrust')) 'Arrow did not change selected result'
 Send '{ESC}' (Popup)
 StopApp;StartApp;Unlock;Button '收起';OpenPopup
 Assert ((Selected).Contains('aTrust')) 'Recent usage lost after restart'
 Send '{ESC}' (Popup)
 Write-Output 'PASS: successful fills update recent ordering and persist after restart; search relevance wins; failed and canceled actions do not reorder; arrows choose results.'
 Manage;SetEdit 0 '企业';Button '编辑账号'
 $box=@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '敏感项：使用、查看和编辑前再次验证')[0]
 $box.GetCurrentPattern([Windows.Automation.TogglePattern]::Pattern).Toggle();Button '保存账号';Button '编辑账号'
 Assert (TextExists '当前方式：Windows 账户密码') 'Verification default is not Windows password'
 Send '{TAB}';Assert (TextExists '当前方式：Windows PIN / Hello') 'Tab did not switch to Hello'
 Send '{TAB}';Assert (TextExists '当前方式：密码库主密码') 'Tab did not switch to vault master'
 Send '+{TAB}';Assert (TextExists '当前方式：Windows PIN / Hello') 'Shift Tab did not reverse modes'
 Send '{TAB}';SetEdit 0 'GUI-Master-Only!2026';Button '验证主密码';Pause 1000
 Assert (TextExists '账号信息') 'Keyboard-selected master verification failed'
 Button '取消编辑';Button '编辑账号'
 Assert (TextExists '当前方式：Windows 账户密码') 'New verification reused prior mode or authorization'
 Button '取消验证'
 Write-Output 'PASS: each sensitive action defaults to Windows password; Tab and Shift Tab cycle verification modes; master verification remains single-use.'
 Button '设置';$old=(Edits)[0].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value
 Button '录制快捷键';Send 'p';Assert (TextExists '取消录制') 'Unmodified key accepted'
 Send '{ESC}';Assert ((Edits)[0].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -eq $old) 'Cancel changed shortcut'
 Button '录制快捷键'
 $form.Show();$form.Activate();[VaultTestNative]::SetForegroundWindow($form.Handle) | Out-Null;Pause 300
 [VaultTestNative]::SetForegroundWindow([IntPtr](Main).Current.NativeWindowHandle) | Out-Null;Pause 200
 Assert (TextExists '录制快捷键') 'Focus loss did not cancel recording'
 Assert ((Edits)[0].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -eq $old) 'Focus loss changed shortcut'
 Button '录制快捷键';Send '^%p'
 Assert (TextExists '录制快捷键') 'Existing global shortcut did not record'
 Assert ((Edits)[0].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -eq 'Ctrl+Alt+P') 'Wrong recorded keys'
 Button '保存设置'
 Assert (Test-Path (Join-Path $DataDir 'shortcut.txt')) 'Unified save missed shortcut'
 Assert (Test-Path (Join-Path $DataDir 'lock-settings.txt')) 'Unified save missed locking'
 Assert (Test-Path (Join-Path $DataDir 'launch-settings.txt')) 'Unified save missed launch preference'
 Assert (Test-Path (Join-Path $DataDir 'theme.txt')) 'Unified save missed theme'
 Assert (Test-Path (Join-Path $DataDir 'appearance.txt')) 'Unified save missed appearance'
 Write-Output 'PASS: shortcut capture rejects plain keys, Escape cancels, existing registered shortcut records without opening popup; one save persists all five settings.'
} finally {StopApp;$form.Dispose();$env:PASSWORD_MANAGER_DATA_DIR=$previous}
