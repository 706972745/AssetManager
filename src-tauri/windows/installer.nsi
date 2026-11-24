; ═══════════════════════════════════════════════════════════════════════════
; AssetManager 安装器模板（自定义 fork，移植自 FontManager 2.1.5 的同名模板）
;
; 来源：Tauri CLI 2.12.x 内嵌的 NSIS 模板（@tauri-apps/cli-win32-x64-msvc 里
;      以 handlebars 原文嵌入）。**不要凭空手写这份文件** —— 升级 Tauri 时的
;      正确做法是重新提取上游模板与这份做 diff，再决定怎么合并。
;      ⚠️ 这类 diff **按「出现次数」（多重集）比，不能按集合比**：像
;         `MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive` 这种行在基线里本来就有多个合法
;         实例（许可页/完成页各一处），集合比会把「删掉一处但别处还留着」误判成「没删」。
;
; 本 fork 只做一件事：**把界面交给主程序画**（路线 C，见下面 `ZFM_ROUTE_C`）。
;   · GUI 安装：.onInit 把主程序解到 $PLUGINSDIR，等它跑完四屏界面，
;     按退出码 0（装好）/ 10（用户取消）收尾；
;   · GUI 卸载：un.onInit 先把主程序拷到 %TEMP%，再等它跑完三屏卸载界面；
;   · 界面起不来（其它退出码）→ **只弹一句人话然后非 0 退出**，不回落原生 MUI 页
;     （回落 = 用户先看自绘界面、再冒出一个系统向导，等于"两个安装界面"）；
;   · /S、/P、/UPDATE 三条语义与上游模板完全同源（更新由主程序内部触发）。
;
; **一个字都没动**的东西（动了就是升级断链 / 数据被清）：
;   · .onInit 的 $INSTDIR 默认值与 RestorePreviousInstallLocation
;   · Section Install 里 WriteRegStr ... "" $INSTDIR、WriteUninstaller、
;     UNINSTKEY 的全部值与 EstimatedSize 计算
;   · 三个 NSIS_HOOK_* 插入点（位置也不能变：POSTINSTALL 必须在
;     StartMenuShortcut 之后、SetAutoClose 之前）
;   · 所有 handlebars 循环（#each …，当前大多为空，但将来加资源/文件关联全靠它们注入；
;     删掉以后加资源会静默丢失，构建不报错）
; ═══════════════════════════════════════════════════════════════════════════
Unicode true
ManifestDPIAware true
; Add in `dpiAwareness` `PerMonitorV2` to manifest for Windows 10 1607+ (note this should not affect lower versions since they should be able to ignore this and pick up `dpiAware` `true` set by `ManifestDPIAware true`)
; Currently undocumented on NSIS's website but is in the Docs folder of source tree, see
; https://github.com/kichik/nsis/blob/5fc0b87b819a9eec006df4967d08e522ddd651c9/Docs/src/attributes.but#L286-L300
; https://github.com/tauri-apps/tauri/pull/10106
ManifestDPIAwareness PerMonitorV2

!if "{{compression}}" == "none"
  SetCompress off
!else
  ; Set the compression algorithm. We default to LZMA.
  SetCompressor /SOLID "{{compression}}"
!endif

; Keep above !include to stay ahead of any plugin command
; see https://github.com/tauri-apps/tauri/pull/15422#discussion_r3289239624
{{#if signed_plugins_path}}
!addplugindir "{{signed_plugins_path}}"
{{/if}}

!include MUI2.nsh
!include FileFunc.nsh
!include x64.nsh
!include WordFunc.nsh
!include "utils.nsh"
!include "FileAssociation.nsh"
!include "Win\COM.nsh"
!include "Win\Propkey.nsh"
!include "Win\RestartManager.nsh"
!include "StrFunc.nsh"
${StrCase}
${StrLoc}

{{#if installer_hooks}}
!include "{{installer_hooks}}"
{{/if}}

!define WEBVIEW2APPGUID "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"

!define MANUFACTURER "{{manufacturer}}"
!define PRODUCTNAME "{{product_name}}"
!define VERSION "{{version}}"
!define VERSIONWITHBUILD "{{version_with_build}}"
!define HOMEPAGE "{{homepage}}"
!define INSTALLMODE "{{install_mode}}"
!define LICENSE "{{license}}"
!define INSTALLERICON "{{installer_icon}}"
!define SIDEBARIMAGE "{{sidebar_image}}"
!define HEADERIMAGE "{{header_image}}"
!define UNINSTALLERICON "{{uninstaller_icon}}"
!define UNINSTALLERHEADERIMAGE "{{uninstaller_header_image}}"
!define MAINBINARYNAME "{{main_binary_name}}"
!define MAINBINARYSRCPATH "{{main_binary_path}}"
!define BUNDLEID "{{bundle_id}}"
!define COPYRIGHT "{{copyright}}"
!define OUTFILE "{{out_file}}"
!define ARCH "{{arch}}"
!define ADDITIONALPLUGINSPATH "{{additional_plugins_path}}"
!define ALLOWDOWNGRADES "{{allow_downgrades}}"
!define DISPLAYLANGUAGESELECTOR "{{display_language_selector}}"
!define INSTALLWEBVIEW2MODE "{{install_webview2_mode}}"
!define WEBVIEW2INSTALLERARGS "{{webview2_installer_args}}"
!define WEBVIEW2BOOTSTRAPPERPATH "{{webview2_bootstrapper_path}}"
!define WEBVIEW2INSTALLERPATH "{{webview2_installer_path}}"
!define MINIMUMWEBVIEW2VERSION "{{minimum_webview2_version}}"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}"
!define MANUKEY "Software\${MANUFACTURER}"
!define MANUPRODUCTKEY "${MANUKEY}\${PRODUCTNAME}"
!define UNINSTALLERSIGNCOMMAND "{{uninstaller_sign_cmd}}"
!define ESTIMATEDSIZE "{{estimated_size}}"
!define STARTMENUFOLDER "{{start_menu_folder}}"

; ═══════════════════════════════════════════════════════════════════════════
; ★ 路线 C（AssetManager）：安装器/卸载器的界面由**主程序自己**画。
;
; 为什么：NSIS 自绘只能做到 75% 保真（照片级右图、墨黑按钮、1px 描边都画不出），
; 独立 Tauri 安装器要 +5MB；主程序里已经有 WebView2、图标与全部前端栈 ——
; 借它画，增量只有几百行 Rust（`src-tauri/src/setup_ui.rs`）。
;
; 谁在做什么：
;   安装：setup.exe（GUI）→ 把 assetmanager.exe 解到 $PLUGINSDIR
;         → ExecWait "assetmanager.exe --installer-ui --setup-exe=... --install-dir=..."
;         → 界面内部再拉起 `setup.exe /S /D=<目标目录>`（引擎只有这一份）
;   卸载：uninstall.exe（GUI）→ 把主程序拷到 %TEMP%\AssetManager-setup-ui
;         → ExecWait "assetmanager.exe --uninstall-ui --install-dir=..."
;         → 界面内部再拉起 `uninstall.exe /S [/PURGE=1]`
;
; 下面这行是**唯一开关**（宏定义是纯文本，tauri 渲染后原样保留，在哪编译都有效；
; 用「标记文件 + !if /FileExists」不行 —— 渲染后的脚本在构建产物目录里，
; `${__FILEDIR__}` 指向那儿，标记文件永远找不到）。
;
; 不打开时所有 ZFM_ROUTE_C 分支都不参与编译，打出来的包 = 上游模板原样
; （GUI 走 MUI 原生页）。名字保留 `ZFM_ROUTE_C` 是为了和 FontManager 的
; 同名模板对照阅读，改名前先确认两边的 diff 还能对上。
; ═══════════════════════════════════════════════════════════════════════════
!define ZFM_ROUTE_C

Var PassiveMode
Var UpdateMode
Var NoShortcutMode
; 路线 C：界面明确要求"建桌面快捷方式"（`/DS`）。与 `NoShortcutMode` 是"要/不要"
; 一对，谁最后被设置谁算数（`.onInit` 里 /NS 先、/DS 后）。
Var ForceDesktop
Var WixMode
Var OldMainBinaryName

Name "${PRODUCTNAME}"
BrandingText "${COPYRIGHT}"
OutFile "${OUTFILE}"

; We don't actually use this value as default install path,
; it's just for nsis to append the product name folder in the directory selector
; https://nsis.sourceforge.io/Reference/InstallDir
!define PLACEHOLDER_INSTALL_DIR "placeholder\${PRODUCTNAME}"
InstallDir "${PLACEHOLDER_INSTALL_DIR}"

VIProductVersion "${VERSIONWITHBUILD}"
VIAddVersionKey "ProductName" "${PRODUCTNAME}"
VIAddVersionKey "FileDescription" "${PRODUCTNAME}"
VIAddVersionKey "LegalCopyright" "${COPYRIGHT}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"

# additional plugins
!addplugindir "${ADDITIONALPLUGINSPATH}"

; Uninstaller signing command
!if "${UNINSTALLERSIGNCOMMAND}" != ""
  !uninstfinalize '${UNINSTALLERSIGNCOMMAND}'
!endif

; Handle install mode, `perUser`, `perMachine` or `both`
!if "${INSTALLMODE}" == "perMachine"
  RequestExecutionLevel admin
!endif

!if "${INSTALLMODE}" == "currentUser"
  RequestExecutionLevel user
!endif

!if "${INSTALLMODE}" == "both"
  !define MULTIUSER_MUI
  !define MULTIUSER_INSTALLMODE_INSTDIR "${PRODUCTNAME}"
  !define MULTIUSER_INSTALLMODE_COMMANDLINE
  !if "${ARCH}" == "x64"
    !define MULTIUSER_USE_PROGRAMFILES64
  !else if "${ARCH}" == "arm64"
    !define MULTIUSER_USE_PROGRAMFILES64
  !endif
  !define MULTIUSER_INSTALLMODE_DEFAULT_REGISTRY_KEY "${UNINSTKEY}"
  !define MULTIUSER_INSTALLMODE_DEFAULT_REGISTRY_VALUENAME "CurrentUser"
  !define MULTIUSER_INSTALLMODEPAGE_SHOWUSERNAME
  !define MULTIUSER_INSTALLMODE_FUNCTION RestorePreviousInstallLocation
  !define MULTIUSER_EXECUTIONLEVEL Highest
  !include MultiUser.nsh
!endif

; Installer icon
!if "${INSTALLERICON}" != ""
  !define MUI_ICON "${INSTALLERICON}"
!endif

; 配色不再覆盖（MUI_BGCOLOR / MUI_TEXTCOLOR 已删）：
; 路线 C 下这些 MUI 页在 GUI 安装里**根本不会渲染**（.onInit 末尾一定 Quit），
; 保留它们只是照应上游模板结构 / 万一路线 C 关掉时的样子。
; 侧栏 / 顶栏 bmp 都已随自绘页一起删除（`sidebar.bmp`、`header.bmp`），
; 不再有 `sidebarImage` 这个配置项。

; Uninstaller icon
!if "${UNINSTALLERICON}" != ""
  !define MUI_UNICON "${UNINSTALLERICON}"
!endif

; Define registry key to store installer language
!define MUI_LANGDLL_REGISTRY_ROOT "HKCU"
!define MUI_LANGDLL_REGISTRY_KEY "${MANUPRODUCTKEY}"
!define MUI_LANGDLL_REGISTRY_VALUENAME "Installer Language"


; 1. 欢迎页：**不在 NSIS 里**
;
; 路线 C 的欢迎屏、安装位置屏、进度屏、完成屏全部由主程序画
; （`assetmanager.exe --installer-ui`，见 setup.html / src/setup/main.ts）。
; 下面的 MUI 页在 GUI 安装里一行都渲染不到（.onInit 会先 Quit），
; 留着只是照应上游模板、以及路线 C 关掉时还能正常装。
;
; 2. License Page (if defined)
!if "${LICENSE}" != ""
  !define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
  !insertmacro MUI_PAGE_LICENSE "${LICENSE}"
!endif

; 3. Install mode (if it is set to `both`)
!if "${INSTALLMODE}" == "both"
  !define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
  !insertmacro MULTIUSER_PAGE_INSTALLMODE
!endif

; 4. 升级 / 降级选择页：**已删除**
;
; 上游模板的 `Page custom PageReinstall PageLeaveReinstall`（"先卸载再装 / 直接覆盖"）
; 属于自绘备用页，随路线 C 一起删了。原因：
;   · 正常路径根本走不到（.onInit 末尾 Quit 掉了，页面比 .onInit 晚）；
;   · 它带一组"点错就卸载"的危险映射（R-18），留着反而多一处能改错的地方。
; 升级/降级判定仍然在 .onInit 里（ALLOWDOWNGRADES = false 时静默拒绝降级），
; 覆盖安装的语义由界面拉起的 `setup.exe /S` 负责 —— 直接覆盖，不先卸。

; 5. Choose install directory page
;
; 这一页是降级路径里「装到 D 盘」的唯一入口，保留 **MUI 原生页**（不覆盖文案、
; 不做写探针、不锁升级目录）：所有自定义回调 `AmDirPre` / `AmDirLeave` 都已删除。
; 路线 C 的安装位置由界面自己选，最终通过 `/D=` 传给引擎。
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_DIRECTORY

; 6. Start menu shortcut page
;
; ★ 这一页**不是废代码，不许删**。它现在恒被跳过，只是因为 tauri.conf.json 里没配
;   `startMenuFolder` → 走 !else 分支 → PRE 回调 `Skip`（Abort）= 界面上看不到。
;   删掉这段的后果：哪天有人填上 startMenuFolder，就变成「用户不能选开始菜单文件夹名」
;   的功能回退 —— 而且是静默回退（构建不报错，装机时才发现少了选项）。
;   同理，`Call CreateOrUpdateStartMenuShortcut` 与 /S 下补建桌面快捷方式的分支也留着。
Var AppStartMenuFolder
!if "${STARTMENUFOLDER}" != ""
  !define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
  !define MUI_STARTMENUPAGE_DEFAULTFOLDER "${STARTMENUFOLDER}"
!else
  !define MUI_PAGE_CUSTOMFUNCTION_PRE Skip
!endif
!insertmacro MUI_PAGE_STARTMENU Application $AppStartMenuFolder

; 7. Installation page
!insertmacro MUI_PAGE_INSTFILES

; 8. Finish page（MUI 原生；路线 C 的 GUI 安装里渲染不到）
;
; Don't auto jump to finish page after installation page,
; because the installation page has useful info that can be used debug any issues with the installer.
!define MUI_FINISHPAGE_NOAUTOCLOSE
; 自绘完成页（`$(AM_FINISH_*)` 那套大标题 / 双行说明 / 运行与桌面快捷方式勾选）
; 已删除：完成屏由主程序的第四屏负责，这里回到 MUI 默认页。
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_FINISH

; Uninstaller Pages
; 1. Confirm uninstall page（MUI 原生）
;
; 上一轮那个自绘的「同时删除设置与缓存」复选框（un.ConfirmShow / un.ConfirmLeave）
; 已删除 —— 卸载确认屏由主程序的 --uninstall-ui 负责。
; `/PURGE=1` 的口子仍然在 un.onInit 里（静默卸载唯一的删数据入口），
; 语义没变，只是不再有一块 NSIS 画出来的复选框。
Var DeleteAppDataCheckboxState
!define MUI_PAGE_CUSTOMFUNCTION_PRE un.SkipIfPassive
!insertmacro MUI_UNPAGE_CONFIRM

; 2. Uninstalling Page
!insertmacro MUI_UNPAGE_INSTFILES

;Languages
{{#each languages}}
!insertmacro MUI_LANGUAGE "{{this}}"
{{/each}}
!insertmacro MUI_RESERVEFILE_LANGDLL
{{#each language_files}}
  !include "{{this}}"
{{/each}}

; 双语文案（am-strings.nsh）已删除：所有 MUI_TEXT_* 覆盖都只服务于被删掉的自绘页；
; 剩下的原生 MUI 页直接用 NSIS 自带的中文/英文翻译。

Function .onInit
  ${GetOptions} $CMDLINE "/P" $PassiveMode
  ${IfNot} ${Errors}
    StrCpy $PassiveMode 1
  ${EndIf}

  ${GetOptions} $CMDLINE "/NS" $NoShortcutMode
  ${IfNot} ${Errors}
    StrCpy $NoShortcutMode 1
  ${EndIf}

  ; `/DS`：路线 C 的界面在**用户勾了**「在桌面创建快捷方式」时给的正向指令。
  ; 只有 `/NS` 的反向逻辑不够：函数里 `$UpdateMode = 1` 会先 Return 跳过创建，
  ; 光"不禁止"并不等于"要建"（RC-9）。两条都给，语义才闭合。
  ; 只在路线 C 的构建里认这个参数 —— 老包一个字节都不受影响。
  !ifdef ZFM_ROUTE_C
  ${GetOptions} $CMDLINE "/DS" $R0
  ${IfNot} ${Errors}
    StrCpy $NoShortcutMode 0
    StrCpy $ForceDesktop 1
  ${EndIf}
  !endif

  ${GetOptions} $CMDLINE "/UPDATE" $UpdateMode
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}

  !if "${DISPLAYLANGUAGESELECTOR}" == "true"
    !insertmacro MUI_LANGDLL_DISPLAY
  !endif

  !insertmacro SetContext

  ${If} $INSTDIR == "${PLACEHOLDER_INSTALL_DIR}"
    ; Set default install location
    !if "${INSTALLMODE}" == "perMachine"
      ${If} ${RunningX64}
        !if "${ARCH}" == "x64"
          StrCpy $INSTDIR "$PROGRAMFILES64\${PRODUCTNAME}"
        !else if "${ARCH}" == "arm64"
          StrCpy $INSTDIR "$PROGRAMFILES64\${PRODUCTNAME}"
        !else
          StrCpy $INSTDIR "$PROGRAMFILES\${PRODUCTNAME}"
        !endif
      ${Else}
        StrCpy $INSTDIR "$PROGRAMFILES\${PRODUCTNAME}"
      ${EndIf}
    !else if "${INSTALLMODE}" == "currentUser"
      StrCpy $INSTDIR "$LOCALAPPDATA\${PRODUCTNAME}"
    !endif

    Call RestorePreviousInstallLocation
  ${EndIf}


  !if "${INSTALLMODE}" == "both"
    !insertmacro MULTIUSER_INIT
  !endif

  ; Abort downgrades if they are disabled, in the modes that skip the reinstall page choice:
  ; silent, passive and update
  !if "${ALLOWDOWNGRADES}" == "false"
  ${If} ${Silent}
  ${OrIf} $PassiveMode = 1
  ${OrIf} $UpdateMode = 1
    ReadRegStr $R0 SHCTX "${UNINSTKEY}" "DisplayVersion"
    nsis_tauri_utils::SemverCompare "${VERSION}" $R0
    Pop $R0
    ; If downgrading
    ${If} $R0 = -1
      ${If} ${Silent}
        System::Call 'kernel32::AttachConsole(i -1)i.r0'
        ${If} $0 <> 0
          System::Call 'kernel32::GetStdHandle(i -11)i.r0'
          System::call 'kernel32::SetConsoleTextAttribute(i r0, i 0x0004)' ; set red color
          FileWrite $0 "$(silentDowngrades)"
        ${EndIf}
      ${Else}
        MessageBox MB_ICONSTOP "$(silentDowngrades)"
      ${EndIf}
      Abort
    ${EndIf}
  ${EndIf}
  !endif

  ; ── 路线 C：GUI 安装 ────────────────────────────────────────────────
  ; NSIS 只做三件事：① 把主程序解到 $PLUGINSDIR（包里唯一的大件）；
  ; ② 等它把四屏界面跑完；③ 按它的退出码决定这次算"装好"还是"用户取消"。
  ; 真正的解包/注册表/快捷方式仍然由它内部拉起的 `setup.exe /S` 完成 —— 引擎只有一份。
  !ifdef ZFM_ROUTE_C
  ${IfNot} ${Silent}
  ${AndIf} $PassiveMode <> 1
  ${AndIf} $UpdateMode <> 1
    ; **不要 SetSilent**：它会把页面不可逆地抑制掉，一旦界面挂掉就变成
    ; 「没人点却自动装完」（2026-11-14 实测踩到）。.onInit 里 Quit 本身就
    ; 足够阻止页面渲染 —— 因为页面比 .onInit 晚。
    SetOutPath "$PLUGINSDIR"
    File "/oname=${MAINBINARYNAME}.exe" "${MAINBINARYSRCPATH}"
    ExecWait '"$PLUGINSDIR\${MAINBINARYNAME}.exe" --installer-ui --setup-exe="$EXEPATH" --install-dir="$INSTDIR"' $R9
    StrCmp $R9 "0" zfm_rc_ok
    StrCmp $R9 "10" zfm_rc_cancel
    ; ⚠️ 其它返回值（界面起不来 / 被杀软拦 / WebView2 缺失 / 建窗失败）：
    ;   **绝对不许回落到原生 MUI 页**。回落等于「用户先看到一个自绘界面，
    ;   然后又冒出一个系统安装向导」—— 两个界面，用户第一反应是"我是不是装了两遍"。
    ;   这里只报一句人话，然后以非 0 退出（update.rs 的唯一判据就是 ExitCode -eq 0，
    ;   非 0 不会被当成"装好了"）。
    ;   `/S` 走不到这里：上面的 ${IfNot} ${Silent} 已经把静默安装排除在外。
    ;   ⚠️ 只用英文：本文件没有 BOM（见 hooks.nsh 的 BOM 注释），`Unicode true` 下
    ;      非 ASCII 字面量会按 ANSI 代码页解析成乱码 —— 这条路本来就极少走到，
    ;      一句干净的英文胜过一句乱码中文。
    MessageBox MB_ICONSTOP|MB_OK "AssetManager Setup could not start its installer window, so nothing was installed.$\r$\n$\r$\nPlease run the installer again. If this keeps happening, check whether your antivirus blocked it."
    SetErrorLevel 1
    Quit
  zfm_rc_cancel:
    SetErrorLevel 10            ; 用户自己取消：不算失败，但也不是"装好了"
    Quit
  zfm_rc_ok:
    SetErrorLevel 0             ; 界面内部已经拉起 `setup.exe /S` 装完了
    Quit
  ${EndIf}
  !endif
FunctionEnd

Section WebView2
  ; Check if Webview2 is already installed and skip this section
  ${If} ${RunningX64}
    ReadRegStr $4 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${Else}
    ReadRegStr $4 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${EndIf}
  ${If} $4 == ""
    ReadRegStr $4 HKCU "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${EndIf}

  ${If} $4 == ""
    ; Webview2 installation
    ;
    ; Skip if updating
    ${If} $UpdateMode <> 1
      !if "${INSTALLWEBVIEW2MODE}" == "downloadBootstrapper"
        Delete "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        DetailPrint "$(webview2Downloading)"
        NSISdl::download "https://go.microsoft.com/fwlink/p/?LinkId=2124703" "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Pop $0
        ${If} $0 == "success"
          DetailPrint "$(webview2DownloadSuccess)"
        ${Else}
          DetailPrint "$(webview2DownloadError)"
          ; ★ 这是**本仓库真正会编译、真正会执行**的 WebView2 失败分支之一
          ;   （INSTALLWEBVIEW2MODE = downloadBootstrapper）。
          ;   显式置 1，不依赖 NSIS 未文档化默认值（实测裸 Abort 在 /S 下退 2，
          ;   但那是实现细节，不能当契约）。update.rs 的唯一判据是 `ExitCode -eq 0`，
          ;   这里必须保证"下载失败 = 非 0"，否则会被当成装好后删包重启。
          SetErrorLevel 1
          Abort "$(webview2AbortError)"
        ${EndIf}
        StrCpy $6 "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Goto install_webview2
      !endif

      !if "${INSTALLWEBVIEW2MODE}" == "embedBootstrapper"
        Delete "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        File "/oname=$TEMP\MicrosoftEdgeWebview2Setup.exe" "${WEBVIEW2BOOTSTRAPPERPATH}"
        DetailPrint "$(installingWebview2)"
        StrCpy $6 "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Goto install_webview2
      !endif

      !if "${INSTALLWEBVIEW2MODE}" == "offlineInstaller"
        Delete "$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe"
        File "/oname=$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe" "${WEBVIEW2INSTALLERPATH}"
        DetailPrint "$(installingWebview2)"
        StrCpy $6 "$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe"
        Goto install_webview2
      !endif

      Goto webview2_done

      install_webview2:
        DetailPrint "$(installingWebview2)"
        ; $6 holds the path to the webview2 installer
        ExecWait "$6 ${WEBVIEW2INSTALLERARGS} /install" $1
        ${If} $1 = 0
          DetailPrint "$(webview2InstallSuccess)"
        ${Else}
          DetailPrint "$(webview2InstallError)"
          ; ★ 另一处真正会执行的 WebView2 失败分支（bootstrapper 下回来了，但装不上）。
          ;   同上：显式非 0，不靠默认值。装了 WebView2 失败的机器上主程序必白屏，
          ;   绝不能让它看起来"装成功"。
          SetErrorLevel 1
          Abort "$(webview2AbortError)"
        ${EndIf}
      webview2_done:
    ${EndIf}
  ${Else}
    ; ⚠️ 下面这一整块（`!if` 到 `!endif`）在本仓库**当前不参与编译**：
    ;   tauri 渲染出的 `MINIMUMWEBVIEW2VERSION` 是 `""`（tauri.conf.json 未配置
    ;   `minimumWebview2Version`），`!if "${MINIMUMWEBVIEW2VERSION}" != ""` 恒假。
    ;   保留它是给"配了最低 WebView2 版本"的构建用的（那时走 EdgeUpdate 的
    ;   /install appguid 通道，失败点是下面的 ExecWait）。
    ;
    ;   ★ 因此**真正保护静默更新的不在这里**，而是 Section WebView2 上半段那两处
    ;   `SetErrorLevel 1` + `Abort`（原行号 :901 下载失败 / :933 安装失败），
    ;   因为本仓库激活的是 INSTALLWEBVIEW2MODE = downloadBootstrapper。
    !if "${MINIMUMWEBVIEW2VERSION}" != ""
      ${VersionCompare} "${MINIMUMWEBVIEW2VERSION}" "$4" $R0
      ${If} $R0 = 1
        update_webview:
          DetailPrint "$(installingWebview2)"
          ${If} ${RunningX64}
            ReadRegStr $R1 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate" "path"
          ${Else}
            ReadRegStr $R1 HKLM "SOFTWARE\Microsoft\EdgeUpdate" "path"
          ${EndIf}
          ${If} $R1 == ""
            ReadRegStr $R1 HKCU "SOFTWARE\Microsoft\EdgeUpdate" "path"
          ${EndIf}
          ${If} $R1 != ""
            ; Chromium updater docs: https://source.chromium.org/chromium/chromium/src/+/main:docs/updater/user_manual.md
            ; Modified from "HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Microsoft EdgeWebView\ModifyPath"
            ExecWait `"$R1" /install appguid=${WEBVIEW2APPGUID}&needsadmin=true` $1
            ${If} $1 = 0
              DetailPrint "$(webview2InstallSuccess)"
            ${Else}
              ; WebView2 装不上 = 主程序起来必白屏，属于最坏的一类失败，必须让上游知道。
              ;
              ; ★ 更正一处曾写错的事实（2.1.7 QA 实测）：旧代码 `MessageBox` + `Quit`
              ;   在 `/S` 下的退出码**不是 0** —— 裸 `Abort`/`Quit` 在 `/S` 下实测退 **2**，
              ;   所以 update.rs 的 `ExitCode -eq 0` 从来没被它骗过。
              ;   真正的病是：`MessageBox` 在 `/S` 下**照常弹窗并阻塞**，而 update.rs 用
              ;   `Start-Process -Wait -PassThru` 等进程退出 → **永久挂死**。
              ;   `SetErrorLevel 1` 的意义是把"失败 = 非 0"写成显式契约，
              ;   不依赖 NSIS 未文档化的默认值（默认值哪天变了就是"报成功但白屏"）。
              ${If} ${Silent}
                System::Call 'kernel32::AttachConsole(i -1)i.r0'
                ${If} $0 <> 0
                  System::Call 'kernel32::GetStdHandle(i -11)i.r0'
                  System::call 'kernel32::SetConsoleTextAttribute(i r0, i 0x0004)' ; set red color
                  FileWrite $0 "$(webview2InstallError)"
                ${EndIf}
                SetErrorLevel 1
                Quit
              ${Else}
                MessageBox MB_ICONEXCLAMATION|MB_ABORTRETRYIGNORE "$(webview2InstallError)" IDIGNORE ignore IDRETRY update_webview
                ; 走到这里 = 用户选了 Abort（Retry 会跳回 update_webview，Ignore 跳 ignore）
                SetErrorLevel 1
                Quit
              ${EndIf}
              ignore:
            ${EndIf}
          ${EndIf}
      ${EndIf}
    !endif
  ${EndIf}
SectionEnd

Section Install
  ; 装之前先验目录（盘符根 / 系统目录 / 用户目录根 → 拒绝；已有别人文件 → 确认）。
  ; 必须在 SetOutPath 之前：SetOutPath 会把目录建出来，之后就分不清"原本就在"了。
  Call am_check_install_dir

  SetOutPath $INSTDIR

  ; 「$INSTDIR\data 与 $INSTDIR\cache 是本程序建的」的凭据，卸载时只认它（见 Section Uninstall）。
  ; 只在两个目录都还不存在（= 绝不是别人的）或者 marker 已经在（= 我们以前的安装）时写，
  ; 否则一律不写：用户自己就有一个 data\ 的目录，不能被我们认领。
  IfFileExists "$INSTDIR\.am-data" am_marker_write
  IfFileExists "$INSTDIR\data" am_marker_skip
  IfFileExists "$INSTDIR\cache" am_marker_skip
  am_marker_write:
    ClearErrors
    FileOpen $0 "$INSTDIR\.am-data" w
    IfErrors am_marker_skip
    FileWrite $0 "${BUNDLEID}$\r$\n"
    FileClose $0
  am_marker_skip:

  !ifmacrodef NSIS_HOOK_PREINSTALL
    !insertmacro NSIS_HOOK_PREINSTALL
  !endif

  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"

  ; Copy main executable
  File "${MAINBINARYSRCPATH}"

  ; Copy resources
  {{#each resources_dirs}}
    CreateDirectory "$INSTDIR\\{{this}}"
  {{/each}}
  {{#each resources}}
    File /a "/oname={{this.[1]}}" "{{no-escape @key}}"
  {{/each}}

  ; Copy external binaries
  {{#each binaries}}
    File /a "/oname={{this}}" "{{no-escape @key}}"
  {{/each}}

  ; Create file associations
  {{#each file_associations as |association| ~}}
    {{#each association.ext as |ext| ~}}
       !insertmacro APP_ASSOCIATE "{{ext}}" "{{or association.name ext}}" "{{association-description association.description ext}}" "$INSTDIR\${MAINBINARYNAME}.exe,0" "Open with ${PRODUCTNAME}" "$INSTDIR\${MAINBINARYNAME}.exe $\"%1$\""
    {{/each}}
  {{/each}}

  ; Register deep links
  {{#each deep_link_protocols as |protocol| ~}}
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}" "URL Protocol" ""
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}" "" "URL:${BUNDLEID} protocol"
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}\DefaultIcon" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}\shell\open\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
  {{/each}}

  ; Create uninstaller
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; Save $INSTDIR in registry for future installations
  WriteRegStr SHCTX "${MANUPRODUCTKEY}" "" $INSTDIR

  !if "${INSTALLMODE}" == "both"
    ; Save install mode to be selected by default for the next installation such as updating
    ; or when uninstalling
    WriteRegStr SHCTX "${UNINSTKEY}" $MultiUser.InstallMode 1
  !endif

  ; Remove old main binary if it doesn't match new main binary name
  ReadRegStr $OldMainBinaryName SHCTX "${UNINSTKEY}" "MainBinaryName"
  ${If} $OldMainBinaryName != ""
  ${AndIf} $OldMainBinaryName != "${MAINBINARYNAME}.exe"
    Delete "$INSTDIR\$OldMainBinaryName"
  ${EndIf}

  ; Save current MAINBINARYNAME for future updates
  WriteRegStr SHCTX "${UNINSTKEY}" "MainBinaryName" "${MAINBINARYNAME}.exe"

  ; Registry information for add/remove programs
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayName" "${PRODUCTNAME}"
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayIcon" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\""
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr SHCTX "${UNINSTKEY}" "Publisher" "${MANUFACTURER}"
  WriteRegStr SHCTX "${UNINSTKEY}" "InstallLocation" "$\"$INSTDIR$\""
  WriteRegStr SHCTX "${UNINSTKEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoModify" "1"
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoRepair" "1"

  ${GetSize} "$INSTDIR" "/M=uninstall.exe /S=0K /G=0" $0 $1 $2
  IntOp $0 $0 + ${ESTIMATEDSIZE}
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD SHCTX "${UNINSTKEY}" "EstimatedSize" "$0"

  !if "${HOMEPAGE}" != ""
    WriteRegStr SHCTX "${UNINSTKEY}" "URLInfoAbout" "${HOMEPAGE}"
    WriteRegStr SHCTX "${UNINSTKEY}" "URLUpdateInfo" "${HOMEPAGE}"
    WriteRegStr SHCTX "${UNINSTKEY}" "HelpLink" "${HOMEPAGE}"
  !endif

  ; Create start menu shortcut
  !insertmacro MUI_STARTMENU_WRITE_BEGIN Application
    Call CreateOrUpdateStartMenuShortcut
  !insertmacro MUI_STARTMENU_WRITE_END

  ; Create desktop shortcut for silent and passive installers
  ; because finish page will be skipped
  ${If} $PassiveMode = 1
  ${OrIf} ${Silent}
    Call CreateOrUpdateDesktopShortcut
  ${EndIf}

  !ifmacrodef NSIS_HOOK_POSTINSTALL
    !insertmacro NSIS_HOOK_POSTINSTALL
  !endif

  ; Auto close this page for passive mode
  ${If} $PassiveMode = 1
    SetAutoClose true
  ${EndIf}
SectionEnd

Function .onInstSuccess
  ; 成功路径**显式**置 0，不要依赖默认值：模板里多处 `Quit`/`Abort` 的退出码取决于
  ; 之前有没有被 SetErrorLevel 改过，而 update.rs 的唯一判据就是 `ExitCode -eq 0`
  ; （0 → 删包 + 重启）。在这里钉死"装完且装好 = 0"，语义才闭合。
  ; 放在最前面：后面的 RunAsUser 属于"装完之后顺带拉起程序"，它失败不代表没装好。
  SetErrorLevel 0

  ; Check for `/R` flag only in silent and passive installers because
  ; GUI installer has a toggle for the user to (re)start the app
  ${If} $PassiveMode = 1
  ${OrIf} ${Silent}
    ${GetOptions} $CMDLINE "/R" $R0
    ${IfNot} ${Errors}
      ${GetOptions} $CMDLINE "/ARGS" $R0
      nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" "$R0"
    ${EndIf}
  ${EndIf}
FunctionEnd

Function un.onInit
  !insertmacro SetContext

  !if "${INSTALLMODE}" == "both"
    !insertmacro MULTIUSER_UNINIT
  !endif

  ; ↑ 上游是无条件 `MUI_UNGETLANGUAGE`（弹「选择语言」小窗）。0.5.7 起和安装侧对齐：
  ;   `displayLanguageSelector = false` 就不弹 —— 否则卸载时用户先看到语言框、
  ;   再看到自绘卸载界面，又是两个窗口。语言退回注册表里上次选的 / 首个语言。
  !if "${DISPLAYLANGUAGESELECTOR}" == "true"
    !insertmacro MUI_UNGETLANGUAGE
  !endif

  ${GetOptions} $CMDLINE "/P" $PassiveMode
  ${IfNot} ${Errors}
    StrCpy $PassiveMode 1
  ${EndIf}

  ${GetOptions} $CMDLINE "/UPDATE" $UpdateMode
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}

  ; `/PURGE=1`：静默卸载时"同时删除数据"的唯一入口。
  ; 模板里 `$DeleteAppDataCheckboxState` 只有确认页（un.ConfirmShow）会置 1，
  ; 而 /S 下页面根本不跑 —— 没有这条口子，路线 C 界面上的勾选框就是个摆设。
  ; 只有在路线 C 的构建里才认这个参数：老包一个字节都不受影响。
  !ifdef ZFM_ROUTE_C
  ${GetOptions} $CMDLINE "/PURGE" $R0
  ${IfNot} ${Errors}
    StrCpy $DeleteAppDataCheckboxState 1
  ${EndIf}
  !endif

  ; ── 路线 C：GUI 卸载 ────────────────────────────────────────────────
  ; 先把主程序拷到 %TEMP% 再跑界面：界面一会儿要拉起 `uninstall.exe /S`，
  ; 那个进程会删 `$INSTDIR\assetmanager.exe` —— 界面自己就是那个文件的话删不掉，
  ; 用户看到的是"卸完了但程序还在"（这类 bug 最难解释，所以从一开始就避开）。
  !ifdef ZFM_ROUTE_C
  ${IfNot} ${Silent}
  ${AndIf} $PassiveMode <> 1
  ${AndIf} $UpdateMode <> 1
    ; 同上：**不要 SetSilent**。界面失败就退回原生卸载页，不许静默把东西删了。
    CreateDirectory "$TEMP\AssetManager-setup-ui"
    CopyFiles /SILENT "$INSTDIR\${MAINBINARYNAME}.exe" "$TEMP\AssetManager-setup-ui\${MAINBINARYNAME}.exe"
    ${If} ${FileExists} "$TEMP\AssetManager-setup-ui\${MAINBINARYNAME}.exe"
      ExecWait '"$TEMP\AssetManager-setup-ui\${MAINBINARYNAME}.exe" --uninstall-ui --install-dir="$INSTDIR"' $R9
      Delete "$TEMP\AssetManager-setup-ui\${MAINBINARYNAME}.exe"
      RMDir "$TEMP\AssetManager-setup-ui"
      ; 界面的 WebView2 数据目录（用完即弃，别在用户 %TEMP% 里留几十 MB）
      RMDir /r "$TEMP\AssetManager-setup-webview"
      StrCmp $R9 "0" zfm_un_rc_ok
      StrCmp $R9 "10" zfm_un_rc_cancel
    ${EndIf}
    ; 界面起不来 / 用户取消：都退回原生卸载页（原页面一行都没删）
    ; ★ 和**安装**侧刻意不对称：安装的回落已经删掉（会出现"两个安装界面"），
    ;   卸载这边保留 —— 界面起不来（WebView2 缺失 / 被杀软拦）时，原生页是用户
    ;   **唯一**能把软件卸掉的路。少一个界面 < 卸不掉。
    Goto zfm_un_rc_done
  zfm_un_rc_cancel:
    SetErrorLevel 10
    Quit
  zfm_un_rc_ok:
    SetErrorLevel 0
    Quit
  zfm_un_rc_done:
  ${EndIf}
  !endif
FunctionEnd

Section Uninstall

  !ifmacrodef NSIS_HOOK_PREUNINSTALL
    !insertmacro NSIS_HOOK_PREUNINSTALL
  !endif

  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"

  ; Delete the app directory and its content from disk
  ; Copy main executable
  Delete "$INSTDIR\${MAINBINARYNAME}.exe"

  ; Delete resources
  {{#each resources}}
    Delete "$INSTDIR\\{{this.[1]}}"
  {{/each}}

  ; Delete external binaries
  {{#each binaries}}
    Delete "$INSTDIR\\{{this}}"
  {{/each}}

  ; Delete app associations
  {{#each file_associations as |association| ~}}
    {{#each association.ext as |ext| ~}}
      !insertmacro APP_UNASSOCIATE "{{ext}}" "{{or association.name ext}}"
    {{/each}}
  {{/each}}

  ; Delete deep links
  {{#each deep_link_protocols as |protocol| ~}}
    ReadRegStr $R7 SHCTX "Software\Classes\\{{protocol}}\shell\open\command" ""
    ${If} $R7 == "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
      DeleteRegKey SHCTX "Software\Classes\\{{protocol}}"
    ${EndIf}
  {{/each}}


  ; Delete uninstaller
  Delete "$INSTDIR\uninstall.exe"

  {{#each resources_ancestors}}
  RMDir /REBOOTOK "$INSTDIR\\{{this}}"
  {{/each}}
  RMDir "$INSTDIR"

  ; Remove shortcuts if not updating
  ${If} $UpdateMode <> 1
    !insertmacro DeleteAppUserModelId

    ; Remove start menu shortcut
    !insertmacro MUI_STARTMENU_GETFOLDER Application $AppStartMenuFolder
    !insertmacro IsShortcutTarget "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
      Delete "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
      RMDir "$SMPROGRAMS\$AppStartMenuFolder"
    ${EndIf}
    !insertmacro IsShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk"
      Delete "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    ${EndIf}

    ; Remove desktop shortcuts
    !insertmacro IsShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$DESKTOP\${PRODUCTNAME}.lnk"
      Delete "$DESKTOP\${PRODUCTNAME}.lnk"
    ${EndIf}
  ${EndIf}

  ; Remove registry information for add/remove programs
  !if "${INSTALLMODE}" == "both"
    DeleteRegKey SHCTX "${UNINSTKEY}"
  !else if "${INSTALLMODE}" == "perMachine"
    DeleteRegKey HKLM "${UNINSTKEY}"
  !else
    DeleteRegKey HKCU "${UNINSTKEY}"
  !endif

  ; Removes the Autostart entry for ${PRODUCTNAME} from the HKCU Run key if it exists.
  ; This ensures the program does not launch automatically after uninstallation if it exists.
  ; If it doesn't exist, it does nothing.
  ; We do this when not updating (to preserve the registry value on updates)
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}"
  ${EndIf}

  ; Delete app data if the checkbox is selected
  ; and if not updating
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    ; Clear the install location $INSTDIR from registry
    DeleteRegKey SHCTX "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty SHCTX "${MANUKEY}"

    ; Clear the install language from registry
    DeleteRegValue HKCU "${MANUPRODUCTKEY}" "Installer Language"
    DeleteRegKey /ifempty HKCU "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty HKCU "${MANUKEY}"

    SetShellVarContext current
    ; 0.5.7 起数据目录在**安装目录下**（`$INSTDIR\data`：索引 / 日志 / 界面偏好 /
    ; WebView2 用户数据），缩略图缓存在 `$INSTDIR\cache`。勾了「同时删除数据」就必须
    ; 连它们一起删，否则用户看到的是"勾了却没删"。
    ; 上面那次 `RMDir "$INSTDIR"` 因为 data / cache 还在而失败，这里删完再补一次。
    ; ⚠️ `screenshots\` 是**用户的素材**（内置截图库），任何情况下都不删。
    ;
    ; ⚠️ `data` / `cache` 是**最常见**的子目录名：安装目录被选成 `D:\我的素材` 时，
    ;    `RMDir /r "$INSTDIR\data"` 会删掉用户自己的 `D:\我的素材\data`。所以递归删之前
    ;    必须先证明这两个目录是本程序建的：
    ;      · `.am-data`：安装时写的 marker（0.5.14 起）；
    ;      · 老版本没有 marker，退而看 `$INSTDIR\data\index.db` —— 索引库只可能是我们写的。
    ;    两条都不成立 → 一个字都不删（只把 $INSTDIR 本身按"非空"留着）。
    StrCpy $R5 0
    IfFileExists "$INSTDIR\.am-data" am_purge_ours
    IfFileExists "$INSTDIR\data\index.db" am_purge_ours
    Goto am_purge_check_done
    am_purge_ours:
      StrCpy $R5 1
    am_purge_check_done:
    ${If} $R5 = 1
      RMDir /r "$INSTDIR\data"
      RMDir /r "$INSTDIR\cache"
      Delete "$INSTDIR\.am-data"
    ${EndIf}
    RMDir "$INSTDIR"
    ; 老版本（≤0.5.6）的两个位置：留着清一遍，免得升级后卸载留一地旧数据
    RmDir /r "$APPDATA\${BUNDLEID}"
    RmDir /r "$LOCALAPPDATA\${BUNDLEID}"
    RmDir /r "$LOCALAPPDATA\${PRODUCTNAME}\data"
  ${EndIf}

  !ifmacrodef NSIS_HOOK_POSTUNINSTALL
    !insertmacro NSIS_HOOK_POSTUNINSTALL
  !endif

  ; Auto close if passive mode or updating
  ${If} $PassiveMode = 1
  ${OrIf} $UpdateMode = 1
    SetAutoClose true
  ${EndIf}
SectionEnd

; ── 安装目录校验（Section Install 的第一件事）─────────────────────────────
; 为什么要有：`setup_check_dir`（Rust 侧 setup_ui.rs）只查「剩余空间够不够」，装到
; `C:\`、`%WINDIR%`、`%USERPROFILE%` 都放行；而卸载侧会递归删 `$INSTDIR\data` /
; `$INSTDIR\cache` —— data / cache 恰好是最常见的子目录名，选错目录就是删用户文件。
; 这里挡三件事：盘符根、Windows 系统目录、用户目录根；再加一条「目录非空且不是
; 我们的安装目录 → 原生确认」。
; ⚠️ 只用 ASCII 字面量：本文件没有 BOM，`Unicode true` 下非 ASCII 会按 ANSI 代码页
;    解析成乱码（同 .onInit 里那条 MessageBox 的注释）。
Function am_check_install_dir
  Push $R6
  Push $R7
  Push $R8
  Push $R9

  ; 归一化：去掉末尾反斜杠（`D:\foo\` → `D:\foo`），`C:\` 这种 3 字符的根保持原样
  StrCpy $R6 $INSTDIR
  am_dir_trim:
    StrLen $R8 $R6
    StrCmp $R8 3 am_dir_trimmed
    StrCpy $R9 $R6 1 -1
    StrCmp $R9 "\" 0 am_dir_trimmed
    StrCpy $R6 $R6 -1
    Goto am_dir_trim
  am_dir_trimmed:

  ; ① 盘符根，如 `C:\`
  StrLen $R8 $R6
  StrCmp $R8 3 0 am_dir_not_root
  StrCpy $R9 $R6 1 -1
  StrCmp $R9 ":" am_dir_reject_root
  am_dir_not_root:

  ; ② Windows / 系统目录
  StrCmp $R6 $WINDIR am_dir_reject_system
  StrCmp $R6 $SYSDIR am_dir_reject_system

  ; ③ 用户目录根（%USERPROFILE%）
  StrCmp $R6 $PROFILE am_dir_reject_profile

  ; ④ 目录已存在且非空，里面又没有我们的 uninstall.exe → 确认
  ;    （升级 / 重装时 uninstall.exe 一定在，所以不会打扰老用户）
  FindFirst $R7 $R9 "$R6\*.*"
  am_dir_scan:
    StrCmp $R9 "" am_dir_scan_empty
    StrCmp $R9 "." am_dir_scan_next
    StrCmp $R9 ".." am_dir_scan_next
    FindClose $R7
    IfFileExists "$R6\uninstall.exe" am_dir_ok
    MessageBox MB_YESNO|MB_ICONEXCLAMATION "This folder is not empty and is not an AssetManager folder:$\r$\n$R6$\r$\n$\r$\nThe program files will be placed next to the existing files.$\r$\n$\r$\nInstall into this folder anyway?" IDYES am_dir_ok
    Goto am_dir_abort
  am_dir_scan_next:
    FindNext $R7 $R9
    Goto am_dir_scan
  am_dir_scan_empty:
    FindClose $R7
    Goto am_dir_ok

  am_dir_reject_root:
    MessageBox MB_ICONSTOP "AssetManager cannot be installed into the root of a drive:$\r$\n$R6$\r$\n$\r$\nPlease pick a subfolder, for example $R6\AssetManager."
    Goto am_dir_abort
  am_dir_reject_system:
    MessageBox MB_ICONSTOP "AssetManager cannot be installed into a Windows system folder:$\r$\n$R6$\r$\n$\r$\nPlease pick another folder."
    Goto am_dir_abort
  am_dir_reject_profile:
    MessageBox MB_ICONSTOP "AssetManager cannot be installed directly into your user profile folder:$\r$\n$R6$\r$\n$\r$\nPlease pick a subfolder, for example $R6\AssetManager."
    Goto am_dir_abort

  am_dir_abort:
    ; 调用点在 SetOutPath 之前，此时一个字节都还没写，直接非 0 退出即可。
    ; 不能用 Abort：Section 里的 Abort 只是跳过本 Section，安装器照样报"成功"。
    SetErrorLevel 1
    Quit

  am_dir_ok:
  Pop $R9
  Pop $R8
  Pop $R7
  Pop $R6
FunctionEnd

Function RestorePreviousInstallLocation
  ReadRegStr $4 SHCTX "${MANUPRODUCTKEY}" ""
  StrCmp $4 "" +2 0
    StrCpy $INSTDIR $4
FunctionEnd

Function Skip
  Abort
FunctionEnd

Function SkipIfPassive
  ${IfThen} $PassiveMode = 1  ${|} Abort ${|}
FunctionEnd
Function un.SkipIfPassive
  ${IfThen} $PassiveMode = 1  ${|} Abort ${|}
FunctionEnd

Function CreateOrUpdateStartMenuShortcut
  ; We used to use product name as MAINBINARYNAME
  ; migrate old shortcuts to target the new MAINBINARYNAME
  StrCpy $R0 0

  !insertmacro IsShortcutTarget "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\$OldMainBinaryName"
  Pop $0
  ${If} $0 = 1
    !insertmacro SetShortcutTarget "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    StrCpy $R0 1
  ${EndIf}

  !insertmacro IsShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\$OldMainBinaryName"
  Pop $0
  ${If} $0 = 1
    !insertmacro SetShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    StrCpy $R0 1
  ${EndIf}

  ${If} $R0 = 1
    Return
  ${EndIf}

  ; Skip creating shortcut if in update mode or no shortcut mode
  ; but always create if migrating from wix
  ${If} $WixMode = 0
    ${If} $UpdateMode = 1
    ${OrIf} $NoShortcutMode = 1
      Return
    ${EndIf}
  ${EndIf}

  !if "${STARTMENUFOLDER}" != ""
    CreateDirectory "$SMPROGRAMS\$AppStartMenuFolder"
    CreateShortcut "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
  !else
    CreateShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\${PRODUCTNAME}.lnk"
  !endif
FunctionEnd

Function CreateOrUpdateDesktopShortcut
  ; We used to use product name as MAINBINARYNAME
  ; migrate old shortcuts to target the new MAINBINARYNAME
  !insertmacro IsShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\$OldMainBinaryName"
  Pop $0
  ${If} $0 = 1
    !insertmacro SetShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Return
  ${EndIf}

  ; Skip creating shortcut if in update mode or no shortcut mode
  ; but always create if migrating from wix
  ; 例外：路线 C 的 `/DS`（用户勾了"在桌面创建快捷方式"）压过上面两条 ——
  ; 勾选是用户的明确指令，覆盖升级也得按它来（RC-9）。
  ${If} $ForceDesktop <> 1
    ${If} $WixMode = 0
      ${If} $UpdateMode = 1
      ${OrIf} $NoShortcutMode = 1
        Return
      ${EndIf}
    ${EndIf}
  ${EndIf}

  CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro SetLnkAppUserModelId "$DESKTOP\${PRODUCTNAME}.lnk"
FunctionEnd
