# 出どころ：tabler-icons

| 項目 | 値 |
|---|---|
| リポジトリ | https://github.com/tabler/tabler-icons |
| 版 | コミット `85e870dbe8680c2d6deefd062122d30ef1544a41` |
| 取ったもの | `icons/outline/` の下の `.svg`（16 MiB 以下）。当てはまる 5491 件から 200 件を種つきで抜き取り（ChaCha8・種 6）。合わせて 119677 バイト |
| 取り方 | `https://raw.githubusercontent.com/tabler/tabler-icons/85e870dbe8680c2d6deefd062122d30ef1544a41/<道>` から取り、tree の一覧の git の blob の SHA-1 を照らした |
| ライセンス | MIT（`LICENSE` はリポジトリの根の写し） |
| 性質 | 線のアイコン（丸い端と継ぎ目） |
| 一覧 | `MANIFEST.jsonl`（道・バイト数・BLAKE3・git の blob） |

作り直し：`cargo run -p sumie-tools -- corpus fetch-real --only tabler-icons`（ADR-59）
