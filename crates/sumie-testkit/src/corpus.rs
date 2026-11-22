//! 照合の集まりの読み込み（`corpus/index.jsonl`・`local/<集まり>/index.jsonl`。corpus/README.md）。
//! ライブラリの試験（見積もり ≥ 実際・ヒープ ≤ 数えた量を集まり全体で照らす。M1 から）と道具が使う。

use std::path::{Path, PathBuf};

use crate::jsonl::Line;

/// 集まりの 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// 根からの相対の道
    pub path: String,
    pub source: String,
    pub category: String,
    /// `in` か `out:<理由>`
    pub scope: String,
    pub blake3: String,
    /// 使う機能の印
    pub features: Vec<String>,
}

impl Entry {
    pub fn in_scope(&self) -> bool {
        self.scope == "in"
    }

    pub fn has(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }
}

/// workspace の根（testkit の場所から）。
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// 根の下の `index.jsonl` を読む（ない・読めない行があれば誤り）。
pub fn entries(root: &Path) -> Result<Vec<Entry>, String> {
    let text = std::fs::read_to_string(root.join("index.jsonl")).map_err(|e| format!("{}：{e}", root.display()))?;
    let mut out = Vec::new();
    for (i, l) in text.lines().enumerate() {
        let line = Line::parse(l).map_err(|e| format!("{} 行：{e}", i + 1))?;
        let s = |k: &str| line.get_str(k).map(str::to_string).ok_or_else(|| format!("{} 行：{k} がない", i + 1));
        out.push(Entry {
            path: s("path")?,
            source: s("source")?,
            category: s("category")?,
            scope: s("scope")?,
            blake3: s("blake3")?,
            features: s("features")?.split(',').filter(|f| !f.is_empty()).map(str::to_string).collect(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_committed_index() {
        let root = workspace_root().join("corpus");
        let all = entries(&root).unwrap();
        assert!(all.len() >= 1700, "{}", all.len());
        assert!(all.iter().all(|e| root.join(&e.path).is_file()));
        assert!(all.iter().any(|e| e.has("filter")));
        assert!(all.iter().all(Entry::in_scope));
    }
}
