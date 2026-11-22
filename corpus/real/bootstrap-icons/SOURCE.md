# 出どころ：bootstrap-icons

| 項目 | 値 |
|---|---|
| リポジトリ | https://github.com/twbs/icons |
| 版 | コミット `829d2bc2b87543c7d781c815ca4f0feee8cf9e45` |
| 取ったもの | `icons/` の下の `.svg`（16 MiB 以下）。当てはまる 2078 件から 200 件を種つきで抜き取り（ChaCha8・種 5）。合わせて 116183 バイト |
| 取り方 | `https://raw.githubusercontent.com/twbs/icons/829d2bc2b87543c7d781c815ca4f0feee8cf9e45/<道>` から取り、tree の一覧の git の blob の SHA-1 を照らした |
| ライセンス | MIT（`LICENSE` はリポジトリの根の写し） |
| 性質 | 塗りのアイコン（偶奇・円弧） |
| 一覧 | `MANIFEST.jsonl`（道・バイト数・BLAKE3・git の blob） |

作り直し：`cargo run -p sumie-tools -- corpus fetch-real --only bootstrap-icons`（ADR-59）
