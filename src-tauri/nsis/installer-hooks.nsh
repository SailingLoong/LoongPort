; LoongPort 的 NSIS 安装钩子，目前承担两件事：
;
; 1. 从 per-user WiX/MSI 迁移到 NSIS 的一次性兼容层。
;    Tauri 自带的 NSIS 模板会检测旧 WiX 安装，但只枚举 HKLM 的 Uninstall 键；
;    LoongPort 的历史 MSI 使用 InstallScope="perUser"，产品登记在 HKCU，因此会被漏掉。
;    这里不用 DisplayName 模糊匹配，而用历史 MSI 的稳定 UpgradeCode 精确枚举相关产品。
;
;    UpgradeCode 来自 wix/per-user-main.wxs 的 {{upgrade_code}} 在正式构建中的展开值。
;    它是已发布 MSI 的持久身份，不能修改或删除；否则老用户无法自动迁移。
;
; 2. 覆盖安装/应用内更新前，等待 LoongPort.exe 真正可写（见
;    WaitForMainBinaryWritable），替代模板「kill + 固定 Sleep 500」的盲等。
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

LangString LoongPortInstallFileLocked ${LANG_ENGLISH} \
  "Setup could not replace LoongPort because it is still in use. Please wait a moment and try again."
LangString LoongPortInstallFileLocked ${LANG_SIMPCHINESE} \
  "LoongPort 仍在使用中，暂时无法完成安装。请稍后重试。"
LangString LoongPortInstallFileLocked ${LANG_TRADCHINESE} \
  "LoongPort 仍在使用中，暫時無法完成安裝。請稍後重試。"
LangString LoongPortInstallFileLocked ${LANG_JAPANESE} \
  "LoongPort が使用中のため、インストールを完了できませんでした。しばらく待ってからもう一度実行してください。"

; 覆盖安装前等待主程序文件真正可写。
;
; 背景：应用内更新链路是 tauri-plugin-updater 的 install() —— 启动安装器后
; 立即 std::process::exit(0)，应用退出与安装器写文件是并发竞速；Tauri 模板
; 的 CheckIfAppIsRunning 只做一次 kill + 固定 Sleep 500ms 就放行写文件，
; 进程退出/杀软持锁超过 500ms 时 File 写入撞 sharing violation，弹出
; 「无法打开要写入的文件 LoongPort.exe」中止/重试/忽略（每次升级必现的
; 那个弹窗）。
;
; 本宏在本文件 PREINSTALL（先于模板的检查与 File 指令）执行验证式等待：
;   1. 发现 LoongPort.exe 在跑就 kill —— 手动双击安装器覆盖安装同样受益；
;   2. 用 CreateFileW(GENERIC_WRITE, 独占, OPEN_EXISTING) 探测目标文件，
;      能打开才放行。探测不创建、不截断；对「进程已死但文件仍被占用」
;      （如杀软扫描持锁）同样有效，这是模板 kill-only 逻辑覆盖不了的；
;   3. 上限 60 × 250ms = 15 秒，超时给出明确提示并中止，而不是退回裸弹窗。
;
; 注意：${INSTALLMODE}/${MAINBINARYNAME} 是模板在 !insertmacro 展开点之后
; 才生效的 defines，因此这段逻辑只能活在宏体内，不能挪到本文件顶层。
!macro WaitForMainBinaryWritable
  !define LoongPortWaitID ${__LINE__}

  ; 全新安装：目标文件不存在，没有可等的锁，直接放行。
  ${If} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
    StrCpy $R7 0

    loongport_wait_loop_${LoongPortWaitID}:
      !if "${INSTALLMODE}" == "currentUser"
        nsis_tauri_utils::FindProcessCurrentUser "${MAINBINARYNAME}.exe"
      !else
        nsis_tauri_utils::FindProcess "${MAINBINARYNAME}.exe"
      !endif
      Pop $R8
      ${If} $R8 = 0
        !if "${INSTALLMODE}" == "currentUser"
          nsis_tauri_utils::KillProcessCurrentUser "${MAINBINARYNAME}.exe"
        !else
          nsis_tauri_utils::KillProcess "${MAINBINARYNAME}.exe"
        !endif
        Pop $R8
      ${EndIf}

      System::Call 'kernel32::CreateFileW(w "$INSTDIR\${MAINBINARYNAME}.exe", i 0x40000000, i 0, p 0, i 3, i 0x80, p 0) p .r8'
      ${If} $R8 <> -1
        System::Call 'kernel32::CloseHandle(p r8)'
        Goto loongport_wait_done_${LoongPortWaitID}
      ${EndIf}

      IntOp $R7 $R7 + 1
      ${If} $R7 >= 60
        MessageBox MB_ICONSTOP|MB_OK "$(LoongPortInstallFileLocked)"
        SetErrorLevel 1
        Abort
      ${EndIf}
      Sleep 250
      Goto loongport_wait_loop_${LoongPortWaitID}

    loongport_wait_done_${LoongPortWaitID}:
  ${EndIf}

  !undef LoongPortWaitID
!macroend

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

  ; 最后一步：确保主程序文件可写再交还模板写文件。
  ; MSI 迁移可能耗时数秒，放在它之后能把「探测通过 → 模板写文件」的窗口压到最小。
  !insertmacro WaitForMainBinaryWritable
!macroend
