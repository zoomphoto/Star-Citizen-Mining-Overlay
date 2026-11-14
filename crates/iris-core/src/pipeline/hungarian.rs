/// Rectangular minimum-cost linear assignment (Jonker-Volgenant / Kuhn-Munkres dual solver).
///
/// Mirrors `Iris.Core.Pipeline.Hungarian.Solve(double[,] cost)` where `rows <= cols`.
pub struct Hungarian;

impl Hungarian {
    /// Solves the minimum-cost assignment problem for an `n x m` matrix (`cost[row * cols + col]`)
    /// where `rows <= cols`. Returns a `Vec<usize>` of length `rows` mapping each row `i` to its
    /// assigned column `j` (`0 <= j < cols`).
    pub fn solve(cost: &[f64], rows: usize, cols: usize) -> Result<Vec<usize>, &'static str> {
        if rows > cols {
            return Err("More rows than columns.");
        }
        if rows == 0 {
            return Ok(Vec::new());
        }
        if cost.len() < rows * cols {
            return Err("Cost buffer smaller than rows * cols.");
        }

        let mut u = vec![0.0f64; rows + 1];
        let mut v = vec![0.0f64; cols + 1];
        let mut p = vec![0usize; cols + 1];
        let mut way = vec![0usize; cols + 1];
        let mut minv = vec![0.0f64; cols + 1];
        let mut used = vec![false; cols + 1];

        for i in 1..=rows {
            p[0] = i;
            let mut j0 = 0usize;
            minv.fill(f64::INFINITY);
            used.fill(false);

            loop {
                used[j0] = true;
                let i0 = p[j0];
                let mut delta = f64::INFINITY;
                let mut j1 = 0usize;
                for j in 1..=cols {
                    if used[j] {
                        continue;
                    }
                    let current = cost[(i0 - 1) * cols + (j - 1)] - u[i0] - v[j];
                    if current < minv[j] {
                        minv[j] = current;
                        way[j] = j0;
                    }
                    if minv[j] < delta {
                        delta = minv[j];
                        j1 = j;
                    }
                }

                for j in 0..=cols {
                    if used[j] {
                        u[p[j]] += delta;
                        v[j] -= delta;
                    } else {
                        minv[j] -= delta;
                    }
                }

                j0 = j1;
                if p[j0] == 0 {
                    break;
                }
            }

            loop {
                let j1 = way[j0];
                p[j0] = p[j1];
                j0 = j1;
                if j0 == 0 {
                    break;
                }
            }
        }

        let mut result = vec![0usize; rows];
        for j in 1..=cols {
            if p[j] != 0 {
                result[p[j] - 1] = j - 1;
            }
        }
        Ok(result)
    }
}

