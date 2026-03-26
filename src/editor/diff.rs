use super::DiffLine;

/// Compute a line-level LCS diff between two slices of strings.
/// Falls back to a simple all-removed / all-added output for very large inputs.
pub(super) fn lcs_diff(old: &[String], new: &[String]) -> Vec<DiffLine> {
    const CAP: usize = 2000;
    if old.len() > CAP || new.len() > CAP {
        let mut r = Vec::with_capacity(old.len() + new.len());
        for l in old {
            r.push(DiffLine::Removed(l.clone()));
        }
        for l in new {
            r.push(DiffLine::Added(l.clone()));
        }
        return r;
    }
    let (m, n) = (old.len(), new.len());
    let mut dp = vec![0u32; (m + 1) * (n + 1)];
    let idx = |i: usize, j: usize| i * (n + 1) + j;
    for i in 1..=m {
        for j in 1..=n {
            dp[idx(i, j)] = if old[i - 1] == new[j - 1] {
                dp[idx(i - 1, j - 1)] + 1
            } else {
                dp[idx(i - 1, j)].max(dp[idx(i, j - 1)])
            };
        }
    }
    let mut result = Vec::new();
    let (mut i, mut j) = (m, n);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && old[i - 1] == new[j - 1] {
            result.push(DiffLine::Context(old[i - 1].clone()));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[idx(i, j - 1)] >= dp[idx(i - 1, j)]) {
            result.push(DiffLine::Added(new[j - 1].clone()));
            j -= 1;
        } else {
            result.push(DiffLine::Removed(old[i - 1].clone()));
            i -= 1;
        }
    }
    result.reverse();
    result
}
