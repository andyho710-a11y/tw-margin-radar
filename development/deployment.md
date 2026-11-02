# 部署與維護手冊（僅供審查，尚未執行）

## 審查後的部署流程

1. 先看 PR checks、build artifact與SHA256；在獨立測試環境確認二進位檔為Linux x86_64 musl靜態檔。交易VM不安裝Rust/GCC、不編譯。
2. 重新量測VM RAM、磁碟及尖峰CPU，確認可容納192MiB工作上限與至少1GiB保留空間。重新核對現有程序狀態，將取樣存於使用者私人維運紀錄。
3. 經使用者確認後，建立專用無登入使用者/群組 `tw-radar`。唯讀程式放 `/opt/tw-margin-radar`（root持有），設定放 `/etc/tw-margin-radar/radar.json`（root:tw-radar 0640）；state `/var/lib/tw-margin-radar` 由tw-radar持有。
4. 網頁僅放 `docs/rust` 的三個靜態檔於 `/var/www/tw-margin-radar`，建立可寫的 `data/` 子目錄。需要既有nginx的讀取群組與檔案權限核對；private state、raw及日誌不可放web root。若無web server，需另審查安裝、TLS、網域與對外防火牆，不能宣稱安裝批次就有公開手機網址。
5. `ops/nginx/radar.conf.example` 預設僅127.0.0.1:8088，先本機驗證；正式手機可查看需獨立網域/TLS反向代理，或選GitHub Pages模式。此設定不覆寫現有server。
6. 以專用使用者執行 `import --from ... --to ...` 補足至少21個連續交易日，再 `preview --date ... --output /var/lib/tw-margin-radar/review.json` 檢查。歷史補抓期間不發正式報告。
7. 核對 `ops/systemd/` units，先 `systemd-analyze verify` 與 `systemd-analyze calendar`；再於隔離測試服務確認CPU/記憶體限制及ProtectHome實際生效、metadata連線被擋。`IPAddressDeny`須主機的systemd/cgroup/BPF支援，不能僅憑設定檔認定已阻擋。不得修改OKX使用者、權限、PM2、啟動設定。
8. 只有使用者批准後才安裝及啟用三個timer。先單次驗收，再啟用每日排程；本次不執行上述主機變更。

## 排程（Asia/Taipei）

| 時間，週一至週五 | 行為 |
|---|---|
| 16:10 | 收集價格，不發布部分報告 |
| 18:40 / 20:10 / 22:10 / 23:10 | 收集同日價格＋融資；日期/完整性未達標則失敗，下個時段自動重試 |
| 23:40 | 檢查完整報告是否到當日交易日，過期則失敗告警 |

官方明列休市、週末跳過。`Persistent=true`會補觸發錯過的timer，但程式依當前台灣時間與交易日閘門執行，不假裝補發過去交易日；漏日使用明確的歷史補抓流程。API每次30秒timeout、最多3次、2/4秒退避；回應上限20MB；每端點間至少400ms。單次服務最長15分鐘、全程序OS檔案鎖、同一交易日不可變報告、原子latest指標。

## 告警

所有失敗均在stderr/journal及private `events.jsonl` 記錄；可用 `journalctl -u 'tw-margin-radar@*'` 與 `systemctl list-timers 'tw-margin-radar*'` 維護。`status.json`記錄最後一次執行狀態；網頁仍顯示最後完整報告並提示日期。

`OnFailure`呼叫獨立的 `ops/alert.py`，預設僅本地結構化事件。經使用者選定目的地與確認後，才將**專用** `RADAR_ALERT_WEBHOOK=https://...` 放 `/etc/tw-margin-radar/alert.env`（0640）。不讀取或重用OKX/Telegram憑證，POST僅service/stage/date/status，不傳例外原文或交易資料。HTTP失敗不設已送標記，可重試；每階段每日去重，另帶Idempotency-Key供接收端去重。送出後崩潰仍可能需接收端去重，不能宣稱跨網路恰好一次。告警服務本身失敗留journal，監控端仍需檢查告警投遞。

## 維護、備份、回復

- 每週檢查磁碟剩餘與增長，為此工作保留至少1GiB餘裕；若低於門檻先停用**雷達自己的timer**並保留現有完整報告，再討論擴容，勿動交易程序。`ops/logrotate/tw-margin-radar`只輪替本專案日誌。
- 每日備份state中的history/reports/calendar及raw（可壓縮存外部備份），記錄hash與實際取得時間。未驗證備份可還原前不刪歷史；不得用新下載覆寫舊快照。磁碟不足時可讓服務失敗拒發，維持前一份完整報告。
- 原始資料更正會拒絕覆寫。建立新的state版本、保留舊原始hash與舊報告，人工核對差異；重新回測後升模型/資料版本，不直接改已發報告。
- 升級：CI建置新artifact、校驗hash、在獨立目錄驗證來源/回測，再在確認後更換本專案二進位檔。回退只回退雷達程式，資料遷移需另行驗證；不執行PM2指令。
- `all_market=true`只影響研究範圍，不增加每股HTTP請求。擴充前比較覆蓋、排除原因、RSS與runtime。
- 實際啟用前驗收：來源日期錯位、斷線、壞JSON/變更欄位、未公布、缺歷史、雙程序鎖、SIGKILL後重啟、磁碟寫滿、記憶體限制、當日重跑、午夜/週末/國定假日、網頁過期、webhook成功/失敗；本地單元測試不等同於VM故障演練。

## 正式發布界線

本分支 `.github/workflows/rust-validate.yml` 只有程式檢查與artifact，沒有schedule、GCP憑證、Pages寫權限或部署步驟；既有V2排程保持原狀。systemd範本目前只是檔案，沒有安裝到VM。若選Actions＋Pages，需另審查持久history位置及發布workflow，不能用Actions cache當唯一歷史來源。
