/// A minimal unified diff between two texts, computed with a longest-common-
/// subsequence table over lines. Returns an empty string when the inputs are equal.
pub fn unified(old: &str, new: &str, path: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let ops = lcs_ops(&a, &b);
    if ops.iter().all(|o| matches!(o, Op::Keep(_))) {
        return String::new();
    }

    let mut out = format!("--- {path}\n+++ {path} (formatted)\n");
    for line in render_hunks(&ops, &a, &b) {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

enum Op {
    Keep(usize),
    Del(usize),
    Ins(usize),
}

fn lcs_ops(a: &[&str], b: &[&str]) -> Vec<Op> {
    let (n, m) = (a.len(), b.len());
    let mut table = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if a[i] == b[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let mut ops = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push(Op::Keep(i));
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            ops.push(Op::Del(i));
            i += 1;
        } else {
            ops.push(Op::Ins(j));
            j += 1;
        }
    }
    while i < n {
        ops.push(Op::Del(i));
        i += 1;
    }
    while j < m {
        ops.push(Op::Ins(j));
        j += 1;
    }
    ops
}

fn render_hunks(ops: &[Op], a: &[&str], b: &[&str]) -> Vec<String> {
    let mut lines = Vec::new();
    for op in ops {
        match op {
            Op::Keep(i) => lines.push(format!(" {}", a[*i])),
            Op::Del(i) => lines.push(format!("-{}", a[*i])),
            Op::Ins(j) => lines.push(format!("+{}", b[*j])),
        }
    }
    lines
}
