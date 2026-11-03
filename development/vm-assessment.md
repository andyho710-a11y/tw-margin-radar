# VM 唯讀容量與隔離評估

量測時間：台灣 2026-11-03 00:19（UTC 2026-11-02 16:19）。目標 instance 數字ID與既有固定 VM 一致。初次容量量測只查 Compute metadata 與執行 SSH stdin 的 `ops/readonly_probe.py`；未建立遠端檔案、未安裝任何套件、未控制 PM2、未讀 API 金鑰、未呼叫交易或通知 API。

| 項目 | 唯讀結果 |
|---|---|
| VM 機型 | e2-micro，共享 CPU；guest 可見2 vCPU |
| CPU / load | 3秒忙碌取樣0.17%；load 1/5/15分為0.01 / 0.02 / 0.00 |
| RAM | 共975 MiB，可用462 MiB |
| swap | 未配置 |
| 根磁碟 | 共9.62 GiB，剩3.94 GiB |
| 既有程序 | PM2 8個註冊項目：3 online、5 stopped |
| `okx-bot` | 查詢時為 stopped / pid=0；此為既有狀態，未操作 |
| 在線程序記憶體 | 約102.0、49.3、21.4 MiB；最後一個CPU快照1.5% |
| 系統服務 | cron、ssh、GCP guest/osconfig agent、chrony、journald、unattended-upgrades等；未觀察到已運行nginx |

這是一個時間點與3秒 CPU 取樣，並非24小時尖峰負載或交易健康證明。主機沒有 swap 且磁碟不大，**暫不建議直接啟用同機正式排程**。可在審查通過後，先以獨立使用者、記憶體192MiB硬上限、CPU20%額度、每日短時批次作受控驗收，實測尖峰 RSS、API失敗恢復、記憶體不足行為、磁碟成長及對既有程序的影響後才決定共用。

結束前台灣00:45再次唯讀取樣：可用RAM465MiB、磁碟仍3.94GiB；8個PM2項目的name/pid/status/restart_count/started_epoch_ms與00:19逐項一致。這是兩次取樣比較，非期間內的連續監控。

## 建議的選擇

1. 最低干擾：GitHub Actions 批次＋既有 GitHub Pages；需另設可靠的歷史持久化與有延遲容忍的排程。現有正式 workflow 保持原狀，本分支只新增讀取權限的驗證 CI。
2. 現有VM受限批次：可評估，但不在VM編譯、不加入現有PM2、不沿用Bot使用者或通知憑證。獨立 static root 與服務；systemd限制需要在該主機實際驗證才算隔離已成立。
3. 獨立VM：當RAM或資料保留超過安全空間，選擇獨立主機；帳號、磁碟、服務故障邊界更清楚。

## 成本估算（規劃假設，非即時報價）

| 方式 | 本工作負載新增預算 |
|---|---|
| 既有VM共用且不擴容 | Compute邊際費用約US$0；磁碟、備份、流量依用量另計，既有VM帳單不等於0 |
| 公開repo GitHub Actions＋Pages | 先以US$0服務費作預算假設，仍受當時方案、使用政策及額度限制 |
| 獨立小型GCP VM | 先預留每月US$10–20，含少量磁碟/流量的粗估；不是從本帳戶帳單或即時價格查得 |
| 告警webhook | 依使用者選擇的提供者計費，尚未指定也未啟用 |

每月按20個交易日、每日四張官方完整報表估資料量，再用實測檔案大小計算；勿用API檔數直接假設空間無上限。部署前以[Google Cloud定價計算機](https://cloud.google.com/products/calculator)核價；此次未建立、升級或購買任何資源。

本地31個交易日實測raw約70.4MB（包含失敗查詢）、正規化歷史22.6MB，合計約3MB/交易日；按250交易日粗估每年約0.75GB，另加日誌/報告/備份。以現有3.94GiB餘裕仍須設保留門檻、外部備份與定期容量檢查，不能無限堆積。程式在收集前已加入磁碟至少1GiB可用的拒收閘門。

## 後續隔離驗收

2026-11-03後續已完成一次性DynamicUser隔離驗收，另見`acceptance.md`及`vm-isolation-evidence.json`。這次驗收有建立並清除暫時systemd單元與專用/run resolver，未安裝正式雷達服務、修改主機DNS或啟用排程；不能將本段與初次純唯讀量測混為同一證據層級。正式共用仍須使用者部署審核。
