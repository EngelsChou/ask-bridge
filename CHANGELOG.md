# 更新日誌 (CHANGELOG)

本專案的所有重要變更皆會記錄於本文件中。

## [Unreleased]

---

## [0.3.24] - 2026-07-27

### Fixed
- `close` 不再把 `taskkill` 的本地化輸出混進自己的 stdout。該訊息以主控台 OEM 字碼頁輸出，被 UTF-8 解讀後成為亂碼，讓 MCP 端回傳如 `���: �w�N�פ�...` 的內容；現在改為捕捉並丟棄輔助程序的輸出，只保留 ask-bridge 自己的訊息。

---

## [0.3.23] - 2026-07-27

### Fixed
- 修復送出較長的多行提示（例如把整個 SQL／程式檔內容貼進提問）時失敗於 `Copilot send button did not become active/enabled` 的問題。輸入框會隨內容長高並被推出視窗上緣（實測 top 由 450 變成 -391），而送出按鈕固定在畫面下方，原本以「像素距離 160 以內」判斷按鈕是否屬於該輸入框的做法會算出 800 以上而找不到按鈕；按鈕本身全程都是啟用狀態。現在改以輸入框往上 8 層的容器包含關係辨識送出按鈕，像素距離僅作為備援且改為即時計算，不再使用打字前後快取的座標。

### Verified
- 以含中文註解、約束與索引的 SQL 建表檔（35 行）實測送出並取回完整說明；並確認 listener 結束後再送長提示同樣正常，無殘留的壞狀態。

---

## [0.3.22] - 2026-07-25

### Fixed
- 模型選單若已經是開啟狀態（Escape 未必關得掉，可見視窗下使用者也可能自行點開後離開），原本會再點一次按鈕而把選單關掉，導致下一次切換失敗於 `model not found in menu`。現在先判斷選單是否已開啟，已開就直接沿用，未開才點擊並容許一次被吞掉的點擊。
- 目前模型按鈕的英文縮寫（`GPT 5.5 Think` 之於 `GPT 5.5 深度思考`）納入名稱正規化，避免對同一個模型做多餘的重新選取。

### Verified
- 以三組鏈路各至少三次切換實測（自動→快速→深度→快速→自動、深度→自動→深度→快速→深度、以及每次開新對話的快速→深度→自動），共 13 次提問全部成功；每一步皆重新展開選單、以 `aria-checked="true"` 的實際項目驗證選到的模型正確。

---

## [0.3.21] - 2026-07-25

### Fixed
- 修復在同一個 M365 對話中連續切換模型會失敗於「Copilot model selector not found」的問題：選定模型後，選單按鈕會改顯示「GPT 5.5 快速版」這類與選單項目不同的字樣，原本的偵測樣式認不出來，導致第二次切換（例如先用固定 `快速回應` 工具、再用 `深度思考` 工具）找不到選單。現在額外以「品牌＋版本」形式（GPT／Claude 開頭）辨識顯示目前模型的按鈕，並把 `快速版`、`深度版`、`思考版` 納入名稱正規化。
- 子選單改為逐一嘗試所有可能的展開列，必要時重新開啟選單，涵蓋該列已改為顯示目前選定模型（如「GPT 5.5 快速版 >」）而非固定顯示「GPT」的情況。

---

## [0.3.20] - 2026-07-25

### Fixed
- 所有可見模式的命令（含 --headless=false 的一般提問）在啟動或重用 Chrome 後都會重新確認視窗位於可見桌面；先前只有 listener 具備此保護，導致沿用背景視窗的可見查詢仍可能停留在螢幕外。

---

## [0.3.19] - 2026-07-25

### Fixed
- 修復先執行背景查詢再啟動  時，M365 Copilot 視窗可能整場停留在螢幕外／最小化而「看不到 Chrome」的問題：listener 於啟動後前 30 秒內每 5 秒檢查一次，受管視窗仍在螢幕外或最小化就重新還原至可見位置；已在螢幕上或超過 30 秒後不再干預，避免影響使用者自行最小化視窗。

---

## [0.3.18] - 2026-07-24

### Fixed
- Microsoft 365 Copilot 模型比對支援跨語言複合名稱：固定工具傳入的「GPT 5.5 Think deeper」現在能對上本地化選單的「GPT 5.5 深度思考」（反向亦然，深度思考／快速回應／自動等模式名稱在複合字串內一律正規化後比對）。
- 模型選單按鈕改為最長等待約 10 秒（冷啟動後選單較晚渲染），修復 Chrome 剛啟動後第一次提問出現「Copilot model selector not found」的問題。

---

## [0.3.17] - 2026-07-24

### Fixed
- 修復新版 Chrome（150+）強化程序 ACL、WMI 讀不到 chrome.exe 命令列時，冷啟動必定失敗於「Chrome is listening on port 9223, but its window process could not be identified」的問題：Chrome 由 ask-bridge 剛啟動並寫入 chrome.pid 記錄後，直接以該記錄（CDP browser id + 唯一 listener PID）作為視窗識別依據，不再依賴命令列掃描。
- `close` 在命令列掃描不可用時改用已驗證的 chrome.pid 記錄識別要關閉的程序，並在和緩關閉約 2 秒未生效時升級為強制結束，修復「Timed out waiting for existing ask-bridge Chrome to stop」。

---

## [0.3.16] - 2026-07-24

### Fixed
- `listen` 互動模式不再於手動登入期間提前逾時：原流程要求頁面在約 45 秒內出現輸入框，使用者尚在輸入帳號密碼／MFA 時 listener 即結束，導致 `Return VS Code` 按鈕永遠不會出現。現在 listener 會在整個 `--timeout` 期間等待登入完成與聊天輸入框就緒（每秒檢查、自動重新選取 Copilot 分頁），完成後才開始注入按鈕。
- `/chat/blocked` 導向改為持續 15 秒以上才判定無 Copilot 授權，避免登入轉址過程的瞬時誤判。
- `Return VS Code` 按鈕恢復「有回覆才能點擊」的判定：M365 尚未產生任何回覆時顯示 `Waiting for response…` 並停用，避免點擊後因擷取不到回覆而失敗。

---

## [0.3.12] - 2026-07-23

### Added
- 新增 `ask-bridge --provider copilot listen` 互動模式：以可見 Chrome 開啟 Microsoft 365 Copilot，在輸入框旁注入 `Return VS Code` 按鈕，等候使用者於 M365 網頁自行加入檔案、截圖或工作內容並完成分析後，將最後一則回覆輸出至 stdout。
- Listener 按鈕會在 M365 SPA 重新渲染後自動補回、生成期間停用，CLI 中止或逾時後由 heartbeat 自動移除。

---

## [0.3.11] - 2026-07-23

### Fixed
- Microsoft 365 Copilot 模型切換支援目前的 GPT 子選單，可選擇 `GPT 5.5 Think deeper` 與 `GPT 5.5 快速回應` 等租戶可見選項。

---

## [0.3.10] - 2026-07-22

### Changed
- 將內嵌的 `mcp-cli` Cargo dependency 改為 `EngelsChou/mcp-cli` fork，並固定至已驗證的 commit；建置與鎖檔不再連線至原始 upstream repository。

---

## [0.3.9] - 2026-07-22

### Added
- Microsoft 365 Copilot 現在支援 `--model`，可選 `Auto`、`Quick response`、`Think deeper`，以及租戶當下在 `More` 選單中可見的具體模型；找不到指定選項時會在送出 prompt 前停止。

### Fixed
- 修正 Microsoft 365 Copilot 富文字輸入框把 `Shift+Enter` 產生的 DOM 空白行重複計入 `innerText`，導致完整的第二次或多行問題被送出前安全校驗誤判而停留在文字框；現在仍要求所有非空行文字與順序一致，只忽略編輯器額外產生的空白行。

### Verified
- 補上空白行正規化回歸測試，並確認內容變更或重複文字仍會被拒絕。
- Chrome 仍固定重用 `%USERPROFILE%\.config\ask-bridge\chrome-profile`；VS Code 與 terminal 啟動的 ask-bridge 共用同一登入狀態，安裝與一般升級不會刪除此 profile。

---

## [0.3.8] - 2026-07-21

### Fixed
- ask-bridge 啟動專用 Chrome 時會同時分離 stdin、stdout 與 stderr；避免 Chrome 繼承 VS Code／MCP 的 prompt stdin pipe，導致 CLI 已退出但 MCP 子程序永遠等不到 `close`、結果無法回傳。

---

## [0.3.7] - 2026-07-21

### Fixed
- 修正 Microsoft 365 Copilot 多行問題遇到換行時提早送出的問題；改以逐行輸入與 `Shift+Enter` 建立換行，並在送出前核對完整字元與換行數。
- 修正 Microsoft 365 Copilot 已完成回答卻未回傳到 VS Code／MCP 的問題；回答偵測會同步追蹤回答容器、動作控制項、最新文字簽章及穩定文字候選。
- 避免多行輸入意外觸發產生後將「停止」控制項誤認為送出按鈕；若明確送出前已開始產生，現在會安全停止並回報錯誤。
- 修正 Windows PowerShell 安裝器原子替換既有檔案時使用無效備份路徑而失敗的問題。
- Microsoft 365 Copilot 帳號或租戶沒有 Copilot Chat 權限並導向 `/chat/blocked` 時，現在會立即回報可操作的錯誤且不會送出問題。

### Added
- Microsoft 365 Copilot 診斷紀錄新增跨程序 request ID、composer 核對資訊與回答完成訊號，方便串接 VS Code／MCP 端到端追蹤，且不記錄問題或回答內容。

---

## [0.3.6] - 2026-07-17

### Fixed
- 修正 Microsoft 365 Copilot 已回答完成，但 VS Code／MCP 仍持續等待的問題；完成與複製控制項現在支援 `role="button"`、內層圖示語意與新版回答容器。

### Added
- 新增隱私安全的 Microsoft 365 Copilot JSONL 診斷紀錄，追蹤 Chrome 啟動、問題送出、等待狀態、完成判定與回答擷取方式；不保存問題、回答內容、Cookie、權杖或登入資料，並在 2 MiB 時自動輪替。

---

## [0.3.5] - 2026-07-17

### Changed
- 直接提問時若偵測到尚未登入，會自動將 ask-bridge 專用 Chrome 切換為可見前景視窗並導向登入頁；登入完成後會自動繼續原本問題，不再要求先另外執行 `ask-bridge login`。登入逾時或視窗無法還原時會停止送出問題並回報原因。

---

## [0.3.4] - 2026-07-17

### Fixed
- 修正 VS Code 重用 ask-bridge 專用 Chrome 時，最小化、尺寸異常或只露出極小區域的視窗被誤判為已顯示；Windows 現在會同步還原視窗、移至可見桌面、重設為可用尺寸並提升至前景。

### Changed
- Release workflow 改為只建置 Windows x86_64 ZIP、`install.exe`、`uninstall.exe` 與 SHA-256 sidecar，不再啟動 Linux 或 macOS runner。

---

## [0.3.3] - 2026-07-16

### Fixed
- 將 npm Trusted Publishing 改為由 repository variable `ASK_BRIDGE_NPM_PUBLISH_ENABLED=true` 明確啟用；未設定 npm 信任關係時仍可獨立完成 GitHub Release 與安裝檔發布。
- 將 CLI、npm 套件、網路安裝腳本與跨平台安裝程式版本同步為 `0.3.3`。

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
