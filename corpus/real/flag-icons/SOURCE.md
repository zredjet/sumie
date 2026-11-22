# 出どころ：flag-icons

| 項目 | 値 |
|---|---|
| リポジトリ | https://github.com/lipis/flag-icons |
| 版 | コミット `ac7e0eb3259eb62aa6100c435607e2fe03896a97` |
| 取ったもの | `flags/4x3/` の下の `.svg`（16 MiB 以下）。当てはまる 271 件の全部。合わせて 1932991 バイト |
| 取り方 | `https://raw.githubusercontent.com/lipis/flag-icons/ac7e0eb3259eb62aa6100c435607e2fe03896a97/<道>` から取り、tree の一覧の git の blob の SHA-1 を照らした |
| ライセンス | MIT（`LICENSE` はリポジトリの根の写し） |
| 性質 | 国旗（紋章の細かい path・グラデーション） |
| 一覧 | `MANIFEST.jsonl`（道・バイト数・BLAKE3・git の blob） |

作り直し：`cargo run -p sumie-tools -- corpus fetch-real --only flag-icons`（ADR-59）
