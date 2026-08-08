; LoongPort 从 per-user WiX/MSI 迁移到 NSIS 的一次性兼容层。
;
; Tauri 自带的 NSIS 模板会检测旧 WiX 安装，但只枚举 HKLM 的 Uninstall 键；
; LoongPort 的历史 MSI 使用 InstallScope="perUser"，产品登记在 HKCU，因此会被漏掉。
; 这里不用 DisplayName 模糊匹配，而用历史 MSI 的稳定 UpgradeCode 精确枚举相关产品。
;
; UpgradeCode 来自 wix/per-user-main.wxs 的 {{upgrade_code}} 在正式构建中的展开值。
; 它是已发布 MSI 的持久身份，不能修改或删除；否则老用户无法自动迁移。
!define LOONGPORT_WIX_UPGRADE_CODE "{f6ae9451-300e-59b9-9081-beb400b6cde1}"

LangString LoongPortMsiMigrationPrompt ${LANG_ENGLISH} \
  "An older MSI installation of LoongPort was found. It must be uninstalled before Setup can continue. Your accounts and settings will be kept. Continue?"
LangString LoongPortMsiMigrationPrompt ${LANG_SIMPCHINESE} \
  "检测到旧版 LoongPort MSI。继续安装前需要先卸载旧版；账号和配置会保留。是否继续？"
LangString LoongPortMsiMigrationPrompt ${LANG_TRADCHINESE} \
  "偵測到舊版 LoongPort MSI。繼續安裝前需要先解除安裝舊版；帳號與設定會保留。是否繼續？"
LangString LoongPortMsiMigrationPrompt ${LANG_JAPANESE} \
  "旧版 LoongPort MSI が見つかりました。セットアップを続行する前にアンインストールします。アカウントと設定は保持されます。続行しますか？"

LangString LoongPortMsiMigrationFailed ${LANG_ENGLISH} \
  "The older LoongPort MSI could not be uninstalled (error $R1). Setup will stop without changing your data. Uninstall LoongPort from Windows Settings, then run Setup again."
LangString LoongPortMsiMigrationFailed ${LANG_SIMPCHINESE} \
  "旧版 LoongPort MSI 卸载失败（错误 $R1）。安装已停止，用户数据未改动。请先在 Windows 设置中卸载 LoongPort，再重新运行安装程序。"
LangString LoongPortMsiMigrationFailed ${LANG_TRADCHINESE} \
  "舊版 LoongPort MSI 解除安裝失敗（錯誤 $R1）。安裝已停止，使用者資料未變更。請先在 Windows 設定中解除安裝 LoongPort，再重新執行安裝程式。"
LangString LoongPortMsiMigrationFailed ${LANG_JAPANESE} \
  "旧版 LoongPort MSI のアンインストールに失敗しました（エラー $R1）。データを変更せずセットアップを中止します。Windows の設定から LoongPort をアンインストールして、もう一度実行してください。"

!macro NSIS_HOOK_PREINSTALL
  ; MsiEnumRelatedProducts 同时覆盖 per-user / per-machine 注册上下文，比遍历 HKCU/HKLM
  ; 并比较显示名称更精确。返回 0 = 找到，1608 = 没有更多相关产品。
  System::Call 'msi::MsiEnumRelatedProductsW(w "${LOONGPORT_WIX_UPGRADE_CODE}", i 0, i 0, w .R0) i .R1'
  ${If} $R1 = 0
    ; 应用内更新已经由用户点过“更新”，且 updater 会传 /UPDATE；不要再弹第二次。
    ; 手动双击 Setup 时则明确告知这次一次性迁移。
    ${If} $UpdateMode != 1
      MessageBox MB_ICONINFORMATION|MB_OKCANCEL "$(LoongPortMsiMigrationPrompt)" IDOK +2
      Abort
    ${EndIf}

    DetailPrint "Removing legacy LoongPort MSI $R0"
    ExecWait '"$SYSDIR\msiexec.exe" /x $R0 /passive /norestart' $R1
    ${If} $R1 != 0
      MessageBox MB_ICONSTOP|MB_OK "$(LoongPortMsiMigrationFailed)"
      SetErrorLevel $R1
      Abort
    ${EndIf}
  ${EndIf}
!macroend
