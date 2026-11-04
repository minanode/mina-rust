//! The `(acc + q) + acc` steps of the variable-base and endomorphism scalar
//! multiplications (`scale_fast` and `endo`), with one field inversion for
//! a whole chain of steps.
//!
//! A step's witness has two slopes, and each slope is a division. Every
//! step starts from the previous step's point, so the divisions can't be
//! batched as they are. Here each step's values are kept as fractions
//! instead, all their denominators are inverted at once (Montgomery's
//! trick), and the values are then recovered. Field arithmetic is exact, so
//! these are the values the step-by-step formulas compute. A division by
//! zero is left to those formulas: the chain is not computed.

use super::field::FieldWitness;

/// The values of one step `(acc + q) + acc` from `acc = (x, y)`, in the
/// order its witness has them: `s1 = (y - yq) / (x - xq)`, its square,
/// `s2 = 2y / (2x + xq - s1^2) - s1`, and the resulting point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DoubleAdd<F> {
    pub s1: F,
    pub s1_squared: F,
    pub s2: F,
    pub x: F,
    pub y: F,
}

/// One step, with two divisions, as `scale_fast` and `endo` compute it.
#[cfg(test)]
fn double_add<F: FieldWitness>((x, y): (F, F), (xq, yq): (F, F)) -> DoubleAdd<F> {
    let s1 = (y - yq) / (x - xq);
    let s1_squared = s1.square();
    let s2 = (y.double() / (x.double() + xq - s1_squared)) - s1;
    let x_res = xq + s2.square() - s1_squared;
    let y_res = ((x - x_res) * s2) - y;
    DoubleAdd {
        s1,
        s1_squared,
        s2,
        x: x_res,
        y: y_res,
    }
}

/// The steps from the affine point `acc`, one for each point of `qs`, each
/// starting from the previous step's point, with one field inversion in
/// all. `None` when a step divides by zero, where the step-by-step formulas
/// panic.
///
/// With the point `(X/Z, Y/Z)`, a step's slopes and point are fractions
/// over `W = E * D`, where `D = X - xq Z` and `E = (2X + xq Z) D^2 - Z N^2`,
/// `N = Y - yq Z`: `s1 = N E / W`, `s2 = M / W` with `M = 2Y D^3 - N E`,
/// `x = Xr / W^2` with `Xr = xq W^2 + M^2 - (N E)^2`, and
/// `y = Yr / (Z W^3)` with `Yr = (X W^2 - Z Xr) M - Y W^3`. The next point
/// is `(Xr Z W, Yr)` over `Z W^3`. `W` is zero exactly when one of the
/// step's two divisors, `x - xq` and `2x + xq - s1^2`, is.
pub fn double_add_chain<F: FieldWitness>(
    (x0, y0): (F, F),
    qs: &[(F, F)],
) -> Option<Vec<DoubleAdd<F>>> {
    struct Fractions<F> {
        ne: F,
        m: F,
        xr: F,
        yr: F,
        w: F,
    }

    let (mut x, mut y, mut z) = (x0, y0, F::one());
    let mut fractions = Vec::with_capacity(qs.len());
    for &(xq, yq) in qs {
        let xq_z = xq * z;
        let n = y - yq * z;
        let d = x - xq_z;
        let d_squared = d.square();
        let e = (x.double() + xq_z) * d_squared - z * n.square();
        let w = e * d;
        if w.is_zero() {
            return None;
        }
        let ne = n * e;
        let m = y.double() * d_squared * d - ne;
        let w_squared = w.square();
        let w_cubed = w_squared * w;
        let xr = xq * w_squared + m.square() - ne.square();
        let yr = (x * w_squared - z * xr) * m - y * w_cubed;
        x = xr * z * w;
        y = yr;
        z *= w_cubed;
        fractions.push(Fractions { ne, m, xr, yr, w });
    }

    // Montgomery's trick: every `1 / w` from the inverse of their product.
    let mut before = Vec::with_capacity(fractions.len());
    let mut product = F::one();
    for f in &fractions {
        before.push(product);
        product *= f.w;
    }
    let mut inverse = product.inverse()?;
    let mut w_inverses = vec![F::zero(); fractions.len()];
    for (i, f) in fractions.iter().enumerate().rev() {
        w_inverses[i] = inverse * before[i];
        inverse *= f.w;
    }

    // `1 / Z` of the next point.
    let mut z_inverse = F::one();
    let steps = fractions
        .iter()
        .zip(w_inverses)
        .map(|(f, w_inverse)| {
            let w_inverse_squared = w_inverse.square();
            z_inverse *= w_inverse_squared * w_inverse;
            let s1 = f.ne * w_inverse;
            DoubleAdd {
                s1,
                s1_squared: s1.square(),
                s2: f.m * w_inverse,
                x: f.xr * w_inverse_squared,
                y: f.yr * z_inverse,
            }
        })
        .collect();
    Some(steps)
}

/// The inverse of each of `values`, or zero for zero, as
/// `inverse().unwrap_or_default()`, with one field inversion in all.
pub fn inverses_or_zero<F: FieldWitness>(values: &[F]) -> Vec<F> {
    let mut before = Vec::with_capacity(values.len());
    let mut product = F::one();
    for v in values {
        before.push(product);
        if !v.is_zero() {
            product *= v;
        }
    }
    // Nonzero: a product of nonzero elements of a field.
    let mut inverse = product.inverse().unwrap_or_default();
    let mut inverses = vec![F::zero(); values.len()];
    for (i, v) in values.iter().enumerate().rev() {
        if !v.is_zero() {
            inverses[i] = inverse * before[i];
            inverse *= v;
        }
    }
    inverses
}

#[cfg(test)]
mod tests {
    use ark_ec::{AffineRepr, CurveGroup};
    use ark_ff::{Field, UniformRand, Zero};
    use mina_curves::pasta::{Fp, Fq, Pallas, Vesta};
    use rand::{rngs::StdRng, Rng, SeedableRng};

    use super::*;

    /// The steps one after the other, with `double_add`.
    fn one_by_one<F: FieldWitness>(acc: (F, F), qs: &[(F, F)]) -> Vec<DoubleAdd<F>> {
        let mut acc = acc;
        qs.iter()
            .map(|&q| {
                let step = double_add(acc, q);
                acc = (step.x, step.y);
                step
            })
            .collect()
    }

    /// Chains of the shapes `scale_fast` and `endo` make, from random points
    /// of both curves: a base point and its negation, and the endomorphism's
    /// images of a point and their negations.
    fn chains<F: FieldWitness, G: AffineRepr<BaseField = F>>(rng: &mut StdRng, endo: F) {
        for len in [1, 2, 10, 64, 255] {
            let base = G::rand(rng);
            let (xb, yb) = base.xy().unwrap();
            let acc = (base + base).into_affine();
            let acc = acc.xy().unwrap();
            let qs: Vec<(F, F)> = (0..len)
                .map(|_| {
                    let x = if rng.gen() { xb * endo } else { xb };
                    let y = if rng.gen() { yb } else { -yb };
                    (x, y)
                })
                .collect();
            let chain = double_add_chain(acc, &qs).unwrap();
            assert_eq!(chain, one_by_one(acc, &qs), "{len} steps");
        }
    }

    #[test]
    fn chain_is_the_step_by_step_formulas() {
        let mut rng = StdRng::seed_from_u64(0);
        let (endo_fq, _) = crate::proofs::transaction::endos::<Fq>();
        let (endo_fp, _) = crate::proofs::transaction::endos::<Fp>();
        for _ in 0..8 {
            chains::<Fp, Pallas>(&mut rng, endo_fp);
            chains::<Fq, Vesta>(&mut rng, endo_fq);
        }
    }

    /// A step that divides by zero, `x = xq`, has no chain, at any place.
    #[test]
    fn division_by_zero_is_left_to_the_formulas() {
        let mut rng = StdRng::seed_from_u64(1);
        let base = Vesta::rand(&mut rng);
        let (xb, yb) = base.xy().unwrap();
        let acc = (base + base).into_affine().xy().unwrap();
        let qs = vec![(xb, yb); 5];
        let chain = double_add_chain(acc, &qs).unwrap();
        for at in 0..qs.len() {
            let mut qs = qs.clone();
            let before = if at == 0 {
                acc
            } else {
                (chain[at - 1].x, chain[at - 1].y)
            };
            qs[at] = (before.0, yb);
            assert_eq!(double_add_chain(acc, &qs), None, "at {at}");
        }
        assert_eq!(double_add_chain::<Fq>(acc, &[]), Some(vec![]));
    }

    #[test]
    fn inverses_or_zero_are_each_inverse() {
        let mut rng = StdRng::seed_from_u64(2);
        let mut values: Vec<Fp> = (0..20).map(|_| Fp::rand(&mut rng)).collect();
        values[0] = Fp::zero();
        values[7] = Fp::zero();
        values[19] = Fp::zero();
        let expected: Vec<Fp> = values
            .iter()
            .map(|v| v.inverse().unwrap_or_default())
            .collect();
        assert_eq!(inverses_or_zero(&values), expected);
        assert_eq!(inverses_or_zero(&[Fp::zero(); 3]), vec![Fp::zero(); 3]);
        assert_eq!(inverses_or_zero::<Fp>(&[]), Vec::<Fp>::new());
    }
}
