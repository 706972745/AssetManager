/**
 * 「这一趟拖放是不是**自己刚从本程序拖出去的**」—— 全局单值。
 *
 * 为什么要它：拖出窗口后由系统拖放接管（OLE）。指针**回到窗口里**时，
 * WebView2 的文件拖入监听（灵感板的导入、以后可能还有别处）也会收到同一批
 * 路径 —— 不拦的话，用户只是想把图拖回来放个标签，结果素材库里凭空多出
 * 一份导入记录（owner 2026-11-20 报「拖回来缩略图消失」时顺藤摸出来的坑）。
 *
 * 路径比较统一小写 + 排序：Explorer 给的路径大小写、正反斜杠可能与库里记的不一致。
 */
let own: string[] | null = null;

export function setOwnDrag(paths: string[] | null) {
  own = paths;
}

export function isOwnDrag(paths: string[] | undefined | null): boolean {
  if (!own || !paths || own.length !== paths.length) return false;
  const a = own.map((p) => p.replace(/\//g, "\\").toLowerCase()).sort();
  const b = paths.map((p) => p.replace(/\//g, "\\").toLowerCase()).sort();
  return a.every((x, i) => x === b[i]);
}
