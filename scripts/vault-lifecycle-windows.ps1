param([Parameter(Mandatory=$true)][string]$ExePath, [Parameter(Mandatory=$true)][string]$DataDir)
. (Join-Path $PSScriptRoot 'windows-test-helpers.ps1')
trap { Write-Output $_.Exception.Message; Write-Output $_.ScriptStackTrace; throw }
Assert (-not (Test-Path (Join-Path $DataDir 'vault.db'))) 'Lifecycle test requires an empty isolated directory'
Assert ([VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents') -eq [IntPtr]::Zero) 'Close existing app before testing'
$previous = $env:PASSWORD_MANAGER_DATA_DIR
$env:PASSWORD_MANAGER_DATA_DIR = $DataDir
$script:process = $null
New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
try {
 StartApp
 Assert (TextExists '创建本地密码库') 'Fresh profile did not show create screen'
 SetEdit 0 'GUI-Master-Only!2026'; SetEdit 1 'mismatch'; Button '创建密码库'
 Assert (TextExists '两次主密码不一致，请重新输入。') 'Mismatched master accepted'
 SetEdit 0 'GUI-Master-Only!2026'; SetEdit 1 'GUI-Master-Only!2026'; Button '创建密码库'; Pause 1600
 Assert (TextExists '0 个账号') 'New vault contains unexpected demo data'
 Write-Output 'PASS: first launch, confirmation validation, create empty encrypted vault.'
 Button '新增账号'; Button '保存账号'
 Assert (TextExists '名称不能为空。') 'Blank title accepted'
 SetEdit 0 'GUI test'; SetEdit 1 'test.user'; SetEdit 2 'Account-Test!2026'; SetEdit 3 'Work'; SetEdit 4 'VPN,test'; SetEdit 5 'https://example.invalid'; SetEdit 6 'test note'
 Button '保存账号'
 Assert (TextExists '1 个账号') 'Create credential failed'
 SetEdit 0 'Work VPN'
 Assert (TextExists '1 个账号') 'Metadata search failed'
 SetEdit 0 'Account-Test!2026'
 Assert (TextExists '0 个账号') 'Password unexpectedly participates in search'
 SetEdit 0 ''

 Button '编辑账号'
 $edits = @(Edits)
 Assert ($edits[1].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -eq 'test.user') 'Username did not round-trip'
 Assert $edits[2].Current.IsPassword 'Credential password is not masked'
 SetEdit 0 'GUI test edited'; SetEdit 2 'Changed-Account!2026'; Button '保存账号'
 Assert (@(Get-ChildItem (Join-Path $DataDir 'backups') -Filter '*.db').Count -ge 1) 'Edit did not create automatic backup'
 Write-Output 'PASS: validation, create/edit records, masked password and encrypted automatic backup.'
 Button '锁定'
 Assert (TextExists '解锁密码库') 'Manual lock failed'
 Unlock 'wrong-master-password'
 Assert (TextExists '解锁密码库') 'Wrong master unlocked vault'
 Unlock
 Assert (TextExists '1 个账号') 'Correct master did not reopen vault'
 SessionLock
 Assert (TextExists '解锁密码库') 'Session lock notification did not clear vault'
 SetEdit 0 'GUI-Master-Only!2026'; Button '解锁'; SessionLock; Pause 1200
 Assert (TextExists '解锁密码库') 'Unlock worker reopened vault after session lock'
 Unlock
 Button '编辑账号'; SessionLock
 Assert (TextExists '解锁密码库') 'Session lock left editor visible'
 Unlock
 $events = [VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents')
 [VaultTestNative]::PostMessage($events,0x218,[IntPtr]4,[IntPtr]::Zero) | Out-Null
 Pause 500
 Assert (TextExists '解锁密码库') 'Suspend notification did not lock vault'
 Unlock
 Write-Output 'PASS: manual/system/suspend notification locking, wrong master refusal and lock during background unlock.'
 Button '加密备份'
 $dialog = [IntPtr]::Zero
 for ($i=0; $i -lt 40; $i++) { $dialog = [VaultTestNative]::FindWindow('#32770','保存加密备份（请选择新文件）'); if ($dialog -ne [IntPtr]::Zero) { break }; Pause 100 }
 Assert ($dialog -ne [IntPtr]::Zero) 'Backup dialog missing before lock test'
 Assert ([Windows.Automation.AutomationElement]::FromHandle($dialog).Current.ProcessId -eq $script:process.Id) 'File dialog belongs to another process'
 SessionLock
 for ($i=0; $i -lt 40 -and [VaultTestNative]::FindWindow('#32770','保存加密备份（请选择新文件）') -ne [IntPtr]::Zero; $i++) { Pause 100 }
 Assert ([VaultTestNative]::FindWindow('#32770','保存加密备份（请选择新文件）') -eq [IntPtr]::Zero) 'Lock did not cancel file dialog'
 Assert (TextExists '解锁密码库') 'File dialog blocked session locking'
 Unlock
 Write-Output 'PASS: system lock cancels this app file dialog and closes the vault.'
 $manual = Join-Path $DataDir 'manual.db'
 Write-Output 'STAGE: manual backup dialog'
 Button '加密备份'; DialogFile '保存加密备份（请选择新文件）' $manual
 for ($i=0; $i -lt 30 -and -not (Test-Path $manual); $i++) { Pause 100 }
 if (-not (Test-Path $manual)) { @(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Text)) | ForEach-Object { Write-Output $_.Current.Name } }
 Assert (Test-Path $manual) 'Manual encrypted backup missing'
 Button '删除账号'
 Assert (TextExists '1 个账号') 'Record removed without confirmation'
 Button '确认删除'
 Assert (TextExists '0 个账号') 'Confirmed delete failed'
 Write-Output 'STAGE: restore dialog'
 Button '恢复备份'; DialogFile '选择要恢复的加密备份' $manual
 Assert (TextExists '解锁密码库') 'Restore did not require master'
 Unlock
 Assert (TextExists '1 个账号') 'Backup did not restore original credential'
 $restoredPath = [IO.File]::ReadAllText((Join-Path $DataDir 'last-vault.txt'))
 Assert ($restoredPath -ne (Join-Path $DataDir 'vault.db')) 'Restore overwrote original vault'
 Write-Output 'PASS: encrypted backup, confirmed deletion and recovery into a separate working copy.'
 StopApp
 StartApp
 Assert (TextExists '解锁密码库') 'Restart reused master password without asking'
 Unlock
 Assert (TextExists '1 个账号') 'Restart lost selected recovered vault'
 Button '编辑账号'
 $edits = @(Edits)
 Assert ($edits[0].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -eq 'GUI test edited') 'Restored record content changed'
 Write-Output 'PASS: restart stays locked, remembers recovered vault and preserves edited record.'
 Button '取消编辑'
 Button '加密备份'
 $dialog = [IntPtr]::Zero
 for ($i=0; $i -lt 40; $i++) { $dialog = [VaultTestNative]::FindWindow('#32770','保存加密备份（请选择新文件）'); if ($dialog -ne [IntPtr]::Zero) { break }; Pause 100 }
 Assert ($dialog -ne [IntPtr]::Zero) 'Backup dialog missing before exit test'
 $events = [VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents')
 [VaultTestNative]::PostMessage($events,0x10,[IntPtr]::Zero,[IntPtr]::Zero) | Out-Null
 Assert ($script:process.WaitForExit(6000)) 'File dialog blocked application exit'
 Write-Output 'PASS: exit cancels this app file dialog.'

} finally {
 StopApp
 $env:PASSWORD_MANAGER_DATA_DIR = $previous
}
