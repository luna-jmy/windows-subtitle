// 幻灯片预览窗（R14 预览排练模式）：显示导入的第 N 页 PNG + 真实字幕条压在上面
// 翻页走全局热键 Ctrl+Alt+→/← 或主界面按钮；真实放映开始时本窗自动关闭
import { useEffect, useState } from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./preview.css";

function Preview() {
  const [page, setPage] = useState(1);
  const [img, setImg] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    const un: Array<() => void> = [];
    (async () => {
      un.push(
        await listen<number>("preview-page", (e) => setPage(Number(e.payload)))
      );
    })();
    return () => un.forEach((f) => f());
  }, []);

  // Esc 关闭排练窗（无边框窗没有关闭按钮）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") invoke("close_preview").catch(() => {});
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    let alive = true;
    (async () => {
      setLoading(true);
      try {
        const data = await invoke<string | null>("read_preview_image", { page });
        if (alive) setImg(data ?? null);
      } catch {
        if (alive) setImg(null);
      } finally {
        if (alive) setLoading(false);
      }
    })();
    return () => {
      alive = false;
    };
  }, [page]);

  return (
    <div className="preview-root">
      {img ? (
        <img className="slide" src={img} alt={`第 ${page} 页`} draggable={false} />
      ) : (
        <div className="slide placeholder">
          <div className="pageno">P{page}</div>
          <div className="hint">{loading ? "加载中…" : "未导入预览图（先在主界面点「导入 PPT」）"}</div>
        </div>
      )}
      <div className="badge">预览排练 · P{page} · Ctrl+Alt+←/→ 翻页 · Esc 退出</div>
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(<Preview />);
