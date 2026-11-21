; AssetManager — NSIS 安装钩子（UTF-8 with BOM，中文注释必须靠 BOM 才能被 NSIS 正确读出来）
;
; 只做一件事：**卸载时清掉双击导入的文件关联**（`file_assoc.rs` 写在 HKCU 的那几项）。
; Tauri 自己的卸载器只管安装目录与它自己写的键，这三组是我们自己加的，必须自己删。
; 不删的后果很具体：用户卸载后 `.S-AssetManager` 仍然显示成"AssetManager 文件"，
; 双击却弹"找不到程序"——这正是 owner 2026-11-19 报的那条。
;
; 另外三条自律（照 FontManager 的 hooks.nsh）：
;   - 不删用户的数据目录（索引 / 截图 / 贴图存档都是用户的东西）；
;   - 不删用户自己建的素材目录、不动系统级 HKCR；
;   - 不删 `Software\<publisher>\AssetManager`（卸载器自己清，这里重复删反而会误伤）。

!include "LogicLib.nsh"

!macro NSIS_HOOK_POSTUNINSTALL
  ; 升级（$UpdateMode = 1）时不删：新版马上就重新注册，删了反而让资源管理器闪一次
  ${If} $UpdateMode <> 1
    ; 扩展名 → ProgID 的那一层（.L/.P 是旧后缀，只读兼容，卸载一样要清）
    DeleteRegKey HKCU "Software\Classes\.D-AssetManager"
    DeleteRegKey HKCU "Software\Classes\.L-AssetManager"
    DeleteRegKey HKCU "Software\Classes\.S-AssetManager"
    DeleteRegKey HKCU "Software\Classes\.P-AssetManager"
    DeleteRegKey HKCU "Software\Classes\.F-AssetManager"
    ; ProgID 自身（open 命令 + 图标）
    DeleteRegKey HKCU "Software\Classes\AssetManager.D"
    DeleteRegKey HKCU "Software\Classes\AssetManager.L"
    DeleteRegKey HKCU "Software\Classes\AssetManager.S"
    DeleteRegKey HKCU "Software\Classes\AssetManager.P"
    DeleteRegKey HKCU "Software\Classes\AssetManager.F"
    ; 图标文件：装到 Program Files 时它们退到 %LOCALAPPDATA%\AssetManager\icons（不在安装目录里，
    ; 卸载器不会替我们清）。装在用户目录时这份本来就在 $INSTDIR 下，RMDir 是空操作，无害。
    RMDir /r "$LOCALAPPDATA\AssetManager\icons"
  ${EndIf}
!macroend
