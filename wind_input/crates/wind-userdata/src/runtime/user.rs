//! 用户词（对照 `wind-store/src/user_words.rs`）。P2 起真源是 `user/<schema>/user.dict.yaml`：
//! 已绑定文本的方案写入经 delta、空闲 / 显式折回进文本；读取叠加「由文本建的段 + delta」。
//! 清空走 `rewrite_text`；导入逐条 Upsert 后立即折回（已有词不挪行位置，新词按行序追加）。

use super::Runtime;
use crate::engine::Op;
use crate::record::{OpKind, Payload, Target};
use crate::snapshot::SnapshotEntry;

/// 与 `wind-store` 的 `USER_WORD_TEXT_MAX_CHARS` / `USER_WORD_CODE_MAX_CHARS` 同值。
pub const USER_TEXT_MAX_CHARS: usize = 10_000;
pub const USER_CODE_MAX_CHARS: usize = 128;

const U: Target = Target::User;

#[derive(Clone, Debug, PartialEq)]
pub struct UserWord {
    pub code: String,
    pub text: String,
    pub weight: i32,
    pub boundary: u64,
    /// 入库顺序（文本行号；尚未折回进文本的新词为 0）。
    pub order: u32,
}

impl From<SnapshotEntry> for UserWord {
    fn from(e: SnapshotEntry) -> UserWord {
        UserWord {
            code: e.code,
            text: e.text,
            weight: e.weight,
            boundary: e.boundary,
            order: e.order,
        }
    }
}

fn check_len(code: &str, text: &str) -> anyhow::Result<()> {
    let n = text.chars().count();
    if n > USER_TEXT_MAX_CHARS {
        anyhow::bail!("词条文本过长：{n} 字（上限 {USER_TEXT_MAX_CHARS}）");
    }
    let n = code.chars().count();
    if n > USER_CODE_MAX_CHARS {
        anyhow::bail!("编码过长：{n} 字符（上限 {USER_CODE_MAX_CHARS}）");
    }
    Ok(())
}

impl Runtime {
    /// 新增或合并一个用户词的操作：已存在 ⇒ 权重取 max、旧边界为 0 时补上（无变化返回 None）。
    fn user_upsert_op(
        &self,
        schema: &str,
        code: &str,
        text: &str,
        weight: i32,
        boundary: u64,
    ) -> Option<Op> {
        let (w, b) = match self.ud.get(U, schema, code, text) {
            Some(e) => {
                let w = e.weight.max(weight);
                let b = if e.boundary != 0 {
                    e.boundary
                } else {
                    boundary
                };
                if w == e.weight && b == e.boundary {
                    return None;
                }
                (w, b)
            }
            None => (weight, boundary),
        };
        Some(Op::new(
            U,
            schema,
            OpKind::Upsert,
            code,
            text,
            Payload::Weight(w),
            b,
        ))
    }

    pub fn add_user_word(
        &self,
        schema: &str,
        code: &str,
        text: &str,
        weight: i32,
        boundary: u64,
    ) -> anyhow::Result<()> {
        check_len(code, text)?;
        if let Some(op) = self.user_upsert_op(schema, code, text, weight, boundary) {
            self.ud.apply_batch(&[op])?;
        }
        self.bump_words(schema);
        Ok(())
    }

    pub fn remove_user_word(&self, schema: &str, code: &str, text: &str) -> anyhow::Result<()> {
        if self.ud.get(U, schema, code, text).is_some() {
            self.ud
                .apply(U, schema, OpKind::Delete, code, text, Payload::None, 0)?;
        }
        self.bump_words(schema);
        Ok(())
    }

    /// 只改权重；不存在返回 false、不新建。
    pub fn update_user_word_weight(
        &self,
        schema: &str,
        code: &str,
        text: &str,
        weight: i32,
    ) -> anyhow::Result<bool> {
        let Some(e) = self.ud.get(U, schema, code, text) else {
            return Ok(false);
        };
        self.ud.apply(
            U,
            schema,
            OpKind::SetWeight,
            code,
            text,
            Payload::Weight(weight),
            e.boundary,
        )?;
        self.bump_words(schema);
        Ok(true)
    }

    pub fn user_word(&self, schema: &str, code: &str, text: &str) -> Option<UserWord> {
        self.ud.get(U, schema, code, text).map(UserWord::from)
    }

    pub fn user_words(&self, schema: &str, code: &str) -> Vec<UserWord> {
        self.ud
            .exact(U, schema, code)
            .into_iter()
            .map(UserWord::from)
            .collect()
    }

    pub fn for_each_user_word(
        &self,
        schema: &str,
        prefix: &str,
        f: &mut dyn FnMut(&UserWord) -> bool,
    ) {
        self.ud
            .scan(U, schema, prefix, &mut |e| f(&UserWord::from(e)));
    }

    pub fn count_user_words(&self, schema: &str) -> usize {
        let mut n = 0;
        self.ud.scan(U, schema, "", &mut |_| {
            n += 1;
            true
        });
        n
    }

    /// 清空：已绑定文本 ⇒ 文件重写为空表（§3.8 整文件替换）；否则整段替换为空。
    pub fn clear_user_words(&self, schema: &str) -> anyhow::Result<usize> {
        let n = self.count_user_words(schema);
        if self.ud.text_binding(U, schema).is_some() {
            self.ud.rewrite_text(U, schema, &[])?;
        } else {
            self.ud.fold_with(U, schema, &mut |_| Vec::new())?;
        }
        self.bump_words(schema);
        Ok(n)
    }

    /// 导入（合并）：已存在取 max 权重、补 0 边界；新词按行序追加。已绑定文本时立即折回进文本——
    /// 导入是用户的显式动作，不等空闲。
    pub fn import_user_rows(&self, schema: &str, rows: &[UserWord]) -> anyhow::Result<usize> {
        for r in rows {
            check_len(&r.code, &r.text)?;
        }
        let ops: Vec<Op> = rows
            .iter()
            .filter_map(|r| self.user_upsert_op(schema, &r.code, &r.text, r.weight, r.boundary))
            .collect();
        if !ops.is_empty() {
            self.ud.apply_batch(&ops)?;
        }
        if self.ud.text_binding(U, schema).is_some() {
            self.ud.fold_text(U, schema)?;
        }
        self.bump_words(schema);
        Ok(rows.len())
    }
}
