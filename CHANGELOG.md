# 更新日誌 (CHANGELOG)

本專案的所有重要變更皆會記錄於本文件中。

## [Unreleased]

---

## [0.3.2] - 2026-07-16

### Fixed
- 修正 Release workflow 在 Linux/macOS PowerShell 解析 Authenticode 錯誤訊息時，因 `$path:` 未使用大括號界定而中止封裝；跨平台建置現在可繼續產生 archive、SHA-256 sidecar 與 Windows 離線安裝程式。
- 將 CLI、npm 套件、網路安裝腳本與跨平台安裝程式版本同步為 `0.3.2`。

---

## [0.3.1] - 2026-07-16

### Fixed
- 將原始碼 clone 與 Agent Skill 入口固定到 `main-add-m365-copilot`；Windows 更新器改為下載最新版 Release 的 `install.exe`，驗證 TLS、有效的 Engels Chou Authenticode 簽章、內嵌憑證 SHA-256 指紋與版本下限後才執行，不再遠端執行 mutable branch script。
- 網路安裝腳本會下載 Release archive 對應的 `.sha256` sidecar，確認檔名及 SHA-256 後才解壓縮。
- PowerShell、macOS/Linux 及離線 Windows 安裝器加入安裝目錄排他鎖與原子替換；macOS/Linux 的版本紀錄另綁定 binary SHA-256，避免並行安裝或中斷造成錯誤降版。
- 離線 `install.exe` 會以 Windows 版本資源（不執行既有 binary）檢查既有版本並預設拒絕降版；只有明確指定 `--allow-downgrade` 才允許回退。
- Release workflow 在提供受信任 Engels Chou PFX 時會簽署並重驗 payload、ZIP 與安裝檔；未提供憑證時仍發布未簽章 Windows 檔案，並保留 SHA-256 與 installer smoke test。已發布 Release 重跑只驗證、不覆寫公開資產，並明確 dispatch 及監看 npm Trusted Publishing workflow。
- 修正從 VS Code MCP 首次登入 Microsoft 365 Copilot 時，新啟動的 Chrome 仍可能沿用背景 profile 的離屏視窗位置，導致工作列只閃現白色視窗卻無法開啟；現在會在確認 Chrome 程序與除錯連接埠身分後，再將 headful 視窗復位到可見桌面。
- 將 CLI、npm 套件與跨平台安裝程式版本同步為 `0.3.1`，並加入分支與版本一致性測試。

---

## [0.3.0] - 2026-07-15

### 🚀 新增 (Added)
- Microsoft 365 Copilot provider 支援可重複指定的 `--image` 與 `--file`，使用網頁官方「新增內容」→「上傳圖片和檔案」流程傳送截圖、程式碼與文件。
- 支援英文、繁體中文與簡體中文的 M365 附件控制項，並補齊常見程式碼、設定檔、Office 文件及圖片 MIME type。

### 🔧 修復 (Fixed)
- 上傳後會驗證附件指示器、檔名、進度與錯誤提示，附件消失或租戶禁止上傳時不再送出 prompt。
- Copilot composer 改用可信鍵盤操作清空可見輸入框並讀回確認，附件上傳後再恢復焦點；每次請求會先 fail closed 拒絕既有附件，並在輸入 prompt 前後精確驗證本次附件數量，避免舊草稿、截圖或檔案混入錯誤請求。
- Copilot 送出按鈕限制於 composer 所屬 form、wrapper 或鄰近區域，避免誤按頁面其他 submit 控制項。
- 登入狀態偵測補齊英文、繁體中文與簡體中文的登入、登出及帳戶控制項，降低第一次背景啟動後無法切換至可見登入流程的情況。
- 安裝、更新、npm postinstall、網站與 Release 來源統一指向 `EngelsChou/ask-bridge`，避免 Copilot fork 被 upstream 版本覆蓋。
- Windows 解除安裝改用隱藏 PowerShell 等待父程序結束後重試自刪，並新增隔離的靜默與延遲按 Enter 互動式 install/uninstall smoke test，確認安裝目錄可完整移除。

### 📚 文件 (Documentation)
- 更新中英文 README、快速開始與 ask-bridge Skill，補充 M365 圖片／文件範例、Microsoft 官方格式、公司租戶授權與 IT 原則限制。
- Windows 檔案版本資訊與應用程式清單發行者設為 `Engels Chou`；Release workflow 可在提供受信任 PFX secrets 時強制 Authenticode 簽章，未提供憑證時仍會明確維持未簽章狀態。

---

## [0.2.5] - 2026-07-10

### 🚀 新增 (Added)
- 新增 `ask-bridge` 問答命令的等待逾時參數（`--timeout`），可自訂回應等待秒數。
- 調整預設回應等待逾時為 `300` 秒（`--timeout` 預設值），降低長時間等待中斷機率。
- `ask-bridge login` 新增登入頁面背景輪詢完成檢測，減少手動切換視窗等待時間。
- Windows 安裝腳本新增本地建構安裝模式（`install.ps1 -Local`），可直接安裝 `target\\release\\ask-bridge.exe`，並保留 `ask.exe` alias 安裝。

### 🔧 修復 (Fixed)
- 修正 npm publish 版本對齊流程，避免在重複發佈時版本比對失敗。
- 修正 CI 平台條件，避免在 Windows 專用 parser 測試執行於非 Windows 平台。
- 修正 `bump-and-release` 首版 SOP 與版本搜尋流程，提升版本升級一致性。

---

## [0.2.3] - 2026-07-10

### 🔧 修復 (Fixed)
- 在 Windows 安裝流程加入 Node.js 版本預檢，限制 `node --version` 至 `^20.19.0`、`^22.12.0` 或 `>=23.0.0`，避免安裝後才遇到 MCP 相容性錯誤。
- 在 CLI 啟動前加入 Node.js Runtime 檢查，若版本不符合 chrome-devtools-mcp 要求，提前中止並輸出可行動的錯誤訊息（含重開終端與安裝建議）。
- 補齊 Node 版本判斷的單元測試：覆蓋支援邊界值與錯誤格式，降低版本不相容回歸風險。

---

## [0.2.2] - 2026-07-10

### 🚀 新增 (Added)
- **Claude（claude.ai）provider 支援**：新增 `--provider claude`，透過 Chrome 自動化 claude.ai 網頁送出 prompt 並取回回覆，與 ChatGPT / Gemini 採相同架構。支援登入偵測（三態 `LoggedIn` / `LoggedOut` / `Unknown`）、分頁重用、`--new` 開新對話、Thread Link 輸出與 `-o` Markdown 檔案輸出。
- Claude 支援 `--image` / `--file` 附件上傳（走既有 DataTransfer 路徑）與 `--model` 模型切換（如 `Sonnet`、`Opus`、`Haiku`，不分大小寫與標點，支援子選單走訪）。
- Selector 依 claude.ai 實站校準：composer 以 `data-testid="chat-input"` 優先；回覆容器採 `.font-claude-response`（`data-is-streaming` 屬性僅掛在最後一則回覆容器，不適合用於訊息計數）。

### 🔧 修復 (Fixed)
- 修正非 Windows 系統（如 macOS、Linux）編譯時，僅在 Windows 平台使用的輔助函數 `parse_windows_netstat_listener_pids` 與 `parse_wmic_column_value` 會產生未使用的編譯警告。

---

## [0.2.1] - 2026-07-10

### 🔧 修復 (Fixed)
- 修正 Windows 執行 `ask-bridge login` 後 Chrome 可能隨命令結束而退出的問題；Chrome 現在會以獨立程序群組與脫離式程序啟動，讓登入工作階段可供後續查詢沿用。
- 強化 `9223` 連接埠的 Chrome 擁有權辨識，加入 ask-bridge 專用標記、PID 紀錄與父程序鏈檢查，避免 Windows Chrome 多程序架構造成誤判。
- 將 ChatGPT 與 Gemini 登入判斷改為 `LoggedIn`、`LoggedOut`、`Unknown` 三態；僅有輸入框時不再誤報登入成功，無法確認時則保留查詢嘗試並顯示警告。
- 多個服務提供者分頁同時存在時優先選取已登入分頁，避免誤選登入頁或未登入分頁。
- Windows `ask-bridge close` 改用 `taskkill /F /PID`，並在程序結束後清理 PID 紀錄。

---

## [0.2.0] - 2026-07-09

### 🚀 新增 (Added)
- 支援 ChatGPT `@Agent` 提示詞輸入；符合 `@名稱 正文` 格式且 Agent 名稱為 1 至 10 個非空白字元時，會先輸入 Agent mention、等待選單出現、按下 Tab 建立 Agent pill，再輸入正文並送出。
- 新增 Agent 提示詞解析與互動流程驗證，涵蓋中文 Agent 名稱、10 字上限、額外空白及不符合格式的輸入。
- 一般 ChatGPT 提示詞與 Gemini 提示詞維持原有送出流程，不套用 Agent 特殊處理。

---

## [0.1.5] - 2026-07-09

### 🔧 修復 (Fixed)
- 修正 ChatGPT 登入判斷過度依賴單一登入按鈕 selector 的問題，改以可見登入控制項、輸入框、帳號選單與登入 URL 綜合判斷。
- 查詢時直接重用已監聽 `9223` 的 ask-bridge Chrome，避免從可見登入模式切換至背景模式時重新啟動 Chrome 並遺失登入狀態。
- 正規化 `--user-data-dir` 命令列比對，支援 Windows 反斜線、引號及參數值以空白分隔的形式。
- 調整 Windows `ask-bridge close` 流程，先嘗試正常終止 Chrome，逾時後再強制關閉。

---

## [0.1.4] - 2026-07-09

### 🔧 修復 (Fixed)
- 修正 Linux/WSL 執行 `ask-bridge --verbose login` 時誤尋找 macOS Chrome 路徑的問題，現在會偵測 `PATH` 中的 `google-chrome` / `google-chrome-stable`，並支援 `/usr/bin/google-chrome` 等常見安裝路徑。
- 修正 `make install` 在 Linux/WSL 環境下的 Chrome 檢查邏輯，避免套用 macOS-only 的 `/Applications/Google Chrome.app` 偵測。

---

## [0.1.3] - 2026-07-08

### 🔧 變更 (Changed)
- 將 `mcp-cli` 依賴從本機路徑更換為指向官方 GitHub 倉庫，使其可以持續同步並拉取最新釋出的 `mcp-cli` 版本（已拉取最新 `v0.2.0` 版本）。

---

## [0.1.2] - 2026-07-08

### 🚀 新增 (Added)
- 建立專利維護指南 [AGENTS.md](file:///G:/Projects/ask-bridge/AGENTS.md)，提供後續 AI 協作者完整的開發架構與相容性修復準則。
- 建立 AI 專用技能定義文件 [.agents/skills/bump-and-release/SKILL.md](file:///G:/Projects/ask-bridge/.agents/skills/bump-and-release/SKILL.md)，詳細說明版本號提升 SOP 與 Git 提交標記步驟。

### 🔧 修復 (Fixed)
- **跨平台 Windows 完整支援**：
  - **Google Chrome 路徑自動偵測**：修正原先硬編碼為 macOS 路徑的問題。現在可在 Windows 環境下自動搜尋系統 `Program Files`、`Program Files (x86)` 與 `%LOCALAPPDATA%` 中的預設安裝位置。
  - **行程與連接埠管理**：
    - Windows 環境中改用 `netstat -ano` 代替 `lsof` 搜尋佔用 `9223` 連接埠的處理程序。
    - 優先使用 `wmic` 取得 Chrome 啟動參數確認其擁有權，若失敗則 Fallback 呼叫 `PowerShell` 命令。
    - 在 Windows 下改用 `taskkill /F` 取代 Unix 的 `kill -TERM` 終止處理程序。
  - **系統限制過濾**：使用 `#[cfg(target_os = "macos")]` 條件編譯，確保 Windows 平台不會觸發 macOS 獨有的 `osascript`（AppleScript）命令。
- **編譯警告優化**：消除 Windows 編譯時因條件編譯產生的未使變數（`unused variables`）警告。
- **程式碼排版美化**：使用 `cargo fmt` 重新校正並排版全專案，確保代碼完全符合 Rustfmt 官方規範。

---

## [0.1.1] - 2024-04-10

- 初始公開釋出版。
- 支援透過 macOS Chrome 的遠端除錯協定（連接埠 `9223`）進行 ChatGPT 與 Gemini 自動化。
- 提供 MCP 連接、背景視窗隱藏與快速問答功能。
