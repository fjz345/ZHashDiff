#[cfg(test)]
use std::sync::{Arc, atomic::AtomicBool};

#[cfg(test)]
use crate::{
    diff_builder::{DiffBuilderOptions, DiffRow, LineContent, build_diff_rows},
    diff_ir::{DiffIR, DiffOp},
    lexer::{LexerDefault, RawToken},
};

#[cfg(test)]
pub struct DiffTestHarness<'a> {
    s1: &'a str,
    s2: &'a str,
    t1: Vec<RawToken>,
    t2: Vec<RawToken>,
    rows: Vec<DiffRow>,
    diff_ir: DiffIR,
}

#[cfg(test)]
impl<'a> DiffTestHarness<'a> {
    pub fn new(
        s1: &'a str,
        s2: &'a str,
        path: Vec<(i32, i32)>,
        options: DiffBuilderOptions,
        estimated_num_rows: usize,
    ) -> Self {
        let t1: Vec<RawToken> = LexerDefault::<RawToken>::new(s1).collect();
        let t2: Vec<RawToken> = LexerDefault::<RawToken>::new(s2).collect();
        let diff_ir = DiffIR::new(&path, false, Arc::new(AtomicBool::new(false)))
            .expect("Failed to create DiffIR");
        let rows = build_diff_rows(
            diff_ir.clone(),
            Some(&t1),
            Some(&t2),
            &options,
            estimated_num_rows,
        );

        Self {
            s1,
            s2,
            t1,
            t2,
            rows,
            diff_ir,
        }
    }

    pub fn diff_irs(&self) -> &DiffIR {
        &self.diff_ir
    }

    pub fn assert_row(&self, idx: usize, l_num: i32, r_num: i32, l_text: &str, r_text: &str) {
        let row = self.rows.get(idx).unwrap_or_else(|| {
            panic!(
                "Expected row at index 


{}, but only {} rows exist.",
                idx,
                self.rows.len()
            )
        });

        assert_row_content(
            idx, row, l_num, r_num, l_text, r_text, &self.t1, &self.t2, self.s1, self.s2,
        );
    }
}

#[cfg(test)]
pub fn assert_row_content(
    idx: usize,
    row: &DiffRow,
    l_line: i32,
    r_line: i32,
    l_text: &str,
    r_text: &str,
    l_tokens: &[RawToken],
    r_tokens: &[RawToken],
    s1: &str,
    s2: &str,
) {
    let extract_details = |content: &LineContent| match content {
        LineContent::Code {
            tokens, line_num, ..
        } => {
            let mut text = String::new();
            let mut debug_tokens = Vec::new();

            for (res, _, _) in tokens {
                // Determine which token array and which index to use
                let (src_tokens, src_text, idx) = match res.operation {
                    DiffOp::Equal(..) | DiffOp::Delete => (
                        l_tokens,
                        s1,
                        res.token_source_idx
                            .expect("Equal/Delete must have source index"),
                    ),
                    DiffOp::Insert => (
                        r_tokens,
                        s2,
                        res.token_target_idx.expect("Insert must have target index"),
                    ),
                };

                let token_raw = &src_tokens[idx as usize];
                let val = &src_text[token_raw.as_ref().span.clone()];

                text.push_str(val);
                debug_tokens.push(format!(
                    "[{:?}: {:?}{}]",
                    res.operation,
                    val.replace('\n', "\\n"),
                    if token_raw.as_ref().kind.is_whitespace() {
                        " (WS)"
                    } else {
                        ""
                    }
                ));
            }
            (text, *line_num, debug_tokens.join(" "))
        }
        LineContent::Void => ("VOID".to_string(), -1, "VOID".to_string()),
        LineContent::Collapsed => ("COLLAPSED".to_string(), -1, "COLLAPSED".to_string()),
    };

    let (act_l_text, act_l_num, act_l_debug) = extract_details(&row.left);
    let (act_r_text, act_r_num, act_r_debug) = extract_details(&row.right);

    if act_l_text != l_text || act_r_text != r_text || act_l_num != l_line || act_r_num != r_line {
        panic!(
            "\nFAIL: Row Index {}\n\
                 {:-<105}\n\
                 {:<5} | {:<5} | {:<40} | {:<5} | {:<40}\n\
                 {:-<105}\n\
                 {:<5} | {:<5} | {:<40?} | {:<5} | {:<40?}\n\
                 {:<5} | {:<5} | {:<40?} | {:<5} | {:<40?}\n\
                 {:-<105}\n\
                 DEBUG TOKENS (ACTUAL):\n\
                 LEFT:  {}\n\
                 RIGHT: {}\n",
            idx,
            "-",
            "SIDE",
            "L-NUM",
            "LEFT TEXT",
            "R-NUM",
            "RIGHT TEXT",
            "-",
            "EXP",
            l_line,
            l_text,
            r_line,
            r_text,
            "ACT",
            act_l_num,
            act_l_text,
            act_r_num,
            act_r_text,
            "-",
            act_l_debug,
            act_r_debug
        );
    }
}
