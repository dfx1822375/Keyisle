param(
 [Parameter(Mandatory=$true)][string]$ExePath,
 [Parameter(Mandatory=$true)][string]$DataDir
)
. (Join-Path $PSScriptRoot 'windows-test-helpers.ps1')
$master='GUI-Master-Only!2026'
Assert (!(Test-Path -LiteralPath $DataDir)) 'Use a new isolated test folder'
Assert ([VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents') -eq [IntPtr]::Zero) 'Close existing Keyisle before testing'
New-Item -ItemType Directory -Path $DataDir | Out-Null
$env:PASSWORD_MANAGER_DATA_DIR=$DataDir
$preferred=(New-Item -ItemType Directory -Path (Join-Path $DataDir 'configured')).FullName
$other=(New-Item -ItemType Directory -Path (Join-Path $DataDir 'one-time')).FullName
$saveTitle='保存加密备份（请选择新文件）'
$restoreTitle='选择要恢复的加密备份'
$folderTitle='选择默认备份目录'
try {
 StartApp -NoFocus
 SetEdit 0 $master; SetEdit 1 $master; Button '创建密码库' 1300
 Copy-Item -LiteralPath (Join-Path $DataDir 'vault.db') -Destination (Join-Path $preferred 'existing-backup.db')
 Button '设置'; Button '备份与维护'
 $pathEdit=@(Edits | Where-Object {$_.Current.Name -eq '默认备份目录'})
 Assert ($pathEdit.Count -eq 1) 'Backup directory edit missing'
 $pathEdit[0].SetFocus()
 Pause 250
 Button '选择文件夹' 450
 $picker=[VaultTestNative]::FindWindow('#32770',$folderTitle)
 Assert ($picker -ne [IntPtr]::Zero) 'Folder picker did not open'
 [VaultTestNative]::PostMessage($picker,0x10,[IntPtr]::Zero,[IntPtr]::Zero) | Out-Null
 Pause 550
 $pathEdit=@(Edits | Where-Object {$_.Current.Name -eq '默认备份目录'})
 $pathEdit[0].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue($preferred)
 Assert (TextExists '有未保存的设置') 'Edited preference did not show the unsaved indicator'
 Button '保存设置' 700
 Assert (!(TextExists '有未保存的设置')) 'Saving preferences did not clear the unsaved indicator'
 $setting=Join-Path $DataDir 'backup-directory.txt'
 Assert ((Get-Content -LiteralPath $setting -Raw) -eq $preferred) 'Default folder did not persist'
 Button '返回账号列表'; Button '备份管理'
 Button '批量选择'
 function HasBackup([string]$name) {
  return @(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::CheckBox) |
      Where-Object {$_.Current.Name.EndsWith($name)}).Count -eq 1
 }
 Assert (HasBackup 'existing-backup.db') 'Existing manual backup missing from the list'
 Button '退出批量选择'
 Button '加密备份'; DialogFile $saveTitle 'first-default.db'
 Assert (Test-Path -LiteralPath (Join-Path $preferred 'first-default.db')) 'First save did not use configured folder'
 $oneOff=Join-Path $other 'one-off.db'
 Button '加密备份'; DialogFile $saveTitle $oneOff
 Assert (Test-Path -LiteralPath $oneOff) 'A different manual save destination did not work'
 Button '加密备份'; DialogFile $saveTitle 'back-to-default.db'
 Assert (Test-Path -LiteralPath (Join-Path $preferred 'back-to-default.db')) 'Save remembered the one-off folder'
 function Restore([string]$path) {
  $before=@(Get-ChildItem -LiteralPath $DataDir -Filter 'restored-*.db' | ForEach-Object Name)
  Button '恢复备份'; DialogFile $restoreTitle $path
  $copies=@(Get-ChildItem -LiteralPath $DataDir -Filter 'restored-*.db' | Where-Object {$_.Name -notin $before})
  $source=if ([IO.Path]::IsPathRooted($path)) {$path} else {Join-Path $preferred $path}
  Assert ($copies.Count -eq 1 -and (Get-FileHash -LiteralPath $copies[0].FullName).Hash -eq (Get-FileHash -LiteralPath $source).Hash) "Restore selected the wrong file: $path"
  Unlock $master
  Button '备份管理'
 }
 Restore 'existing-backup.db'
 Restore $oneOff
 Restore 'back-to-default.db'
 Assert ((Get-Content -LiteralPath $setting -Raw) -eq $preferred) 'One-off paths changed the preference'
 Write-Output 'PASS: folder picker, persisted default, existing list, save and restore defaults after alternate paths.'
} finally {
 $picker=[VaultTestNative]::FindWindow('#32770',$folderTitle)
 if ($picker -ne [IntPtr]::Zero -and $script:process -and [Windows.Automation.AutomationElement]::FromHandle($picker).Current.ProcessId -eq $script:process.Id) {
  [VaultTestNative]::PostMessage($picker,0x10,[IntPtr]::Zero,[IntPtr]::Zero) | Out-Null
 }
 if ($script:process) {StopApp}
}
