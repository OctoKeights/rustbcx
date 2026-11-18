//! Four floats, or four ints, worked on together: what the hottest loops of the encoders are
//! written with, so that the same arithmetic is done on four pixels at a time. (The crate's
//! own.)
//!
//! Each lane is given exactly the operations the plain loop gave one pixel, in the same
//! order, so the results are the same bits. As plain arrays here, the compiler turns the
//! loops over them into SIMD instructions by itself. With the `nightly-simd` feature they are
//! `std::simd` vectors, which says so outright and needs a nightly compiler.

// (Each operation stands for one of ISPC's vector instructions, and is inlined into the kernels
// that use it, as ISPC's would be; without `always` some are not.)
#![allow(clippy::inline_always)]

#[cfg(not(feature = "nightly-simd"))]
mod plain {
    /// Four floats.
    #[derive(Clone, Copy)]
    pub(crate) struct F4(pub(crate) [f32; 4]);

    /// Four ints.
    #[derive(Clone, Copy)]
    pub(crate) struct I4(pub(crate) [i32; 4]);

    impl F4 {
        #[inline(always)]
        pub(crate) fn splat(value: f32) -> F4 {
            F4([value; 4])
        }

        #[inline(always)]
        pub(crate) fn to_array(self) -> [f32; 4] {
            self.0
        }

        #[inline(always)]
        pub(crate) fn sub(self, other: F4) -> F4 {
            F4(std::array::from_fn(|i| self.0[i] - other.0[i]))
        }

        #[inline(always)]
        pub(crate) fn add(self, other: F4) -> F4 {
            F4(std::array::from_fn(|i| self.0[i] + other.0[i]))
        }

        #[inline(always)]
        pub(crate) fn mul(self, other: F4) -> F4 {
            F4(std::array::from_fn(|i| self.0[i] * other.0[i]))
        }

        /// In each lane `self` where it is less than `other`, else `other`.
        #[inline(always)]
        pub(crate) fn lesser(self, other: F4) -> F4 {
            F4(std::array::from_fn(|i| {
                if self.0[i] < other.0[i] {
                    self.0[i]
                } else {
                    other.0[i]
                }
            }))
        }

        /// In each lane `then` where `self` equals `other`, else `otherwise`.
        #[inline(always)]
        pub(crate) fn where_equal(self, other: F4, then: I4, otherwise: I4) -> I4 {
            I4(std::array::from_fn(|i| {
                if self.0[i] == other.0[i] {
                    then.0[i]
                } else {
                    otherwise.0[i]
                }
            }))
        }

        /// In each lane `then` where `self` is less than `other`, else `otherwise`.
        #[inline(always)]
        pub(crate) fn where_less(self, other: F4, then: I4, otherwise: I4) -> I4 {
            I4(std::array::from_fn(|i| {
                if self.0[i] < other.0[i] {
                    then.0[i]
                } else {
                    otherwise.0[i]
                }
            }))
        }
    }

    impl I4 {
        #[inline(always)]
        pub(crate) fn splat(value: i32) -> I4 {
            I4([value; 4])
        }

        #[inline(always)]
        pub(crate) fn to_array(self) -> [i32; 4] {
            self.0
        }
    }
}

#[cfg(feature = "nightly-simd")]
mod plain {
    use std::simd::cmp::{SimdPartialEq, SimdPartialOrd};
    use std::simd::{Select, f32x4, i32x4};

    /// Four floats.
    #[derive(Clone, Copy)]
    pub(crate) struct F4(pub(crate) f32x4);

    /// Four ints.
    #[derive(Clone, Copy)]
    pub(crate) struct I4(pub(crate) i32x4);

    impl F4 {
        #[inline(always)]
        pub(crate) fn splat(value: f32) -> F4 {
            F4(f32x4::splat(value))
        }

        #[inline(always)]
        pub(crate) fn to_array(self) -> [f32; 4] {
            self.0.to_array()
        }

        #[inline(always)]
        pub(crate) fn sub(self, other: F4) -> F4 {
            F4(self.0 - other.0)
        }

        #[inline(always)]
        pub(crate) fn add(self, other: F4) -> F4 {
            F4(self.0 + other.0)
        }

        #[inline(always)]
        pub(crate) fn mul(self, other: F4) -> F4 {
            F4(self.0 * other.0)
        }

        /// In each lane `self` where it is less than `other`, else `other`.
        #[inline(always)]
        pub(crate) fn lesser(self, other: F4) -> F4 {
            F4(self.0.simd_lt(other.0).select(self.0, other.0))
        }

        /// In each lane `then` where `self` equals `other`, else `otherwise`.
        #[inline(always)]
        pub(crate) fn where_equal(self, other: F4, then: I4, otherwise: I4) -> I4 {
            I4(self.0.simd_eq(other.0).select(then.0, otherwise.0))
        }

        /// In each lane `then` where `self` is less than `other`, else `otherwise`.
        #[inline(always)]
        pub(crate) fn where_less(self, other: F4, then: I4, otherwise: I4) -> I4 {
            I4(self.0.simd_lt(other.0).select(then.0, otherwise.0))
        }
    }

    impl I4 {
        #[inline(always)]
        pub(crate) fn splat(value: i32) -> I4 {
            I4(i32x4::splat(value))
        }

        #[inline(always)]
        pub(crate) fn to_array(self) -> [i32; 4] {
            self.0.to_array()
        }
    }
}

pub(crate) use plain::{F4, I4};

impl F4 {
    #[inline(always)]
    pub(crate) fn from_array(values: [f32; 4]) -> F4 {
        #[cfg(not(feature = "nightly-simd"))]
        {
            F4(values)
        }
        #[cfg(feature = "nightly-simd")]
        {
            F4(std::simd::f32x4::from_array(values))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lanes_do_what_one_value_does() {
        let a = F4::from_array([1.0, -2.5, f32::NAN, 4.0]);
        let b = F4::from_array([0.5, -2.5, 3.0, f32::NAN]);
        assert_eq!(a.sub(b).to_array()[..2], [0.5, 0.0]);
        assert_eq!(a.add(b).to_array()[..2], [1.5, -5.0]);
        assert_eq!(a.mul(b).to_array()[..2], [0.5, 6.25]);
        // The lesser, as `if a < b { a } else { b }` has it: the second where either is not
        // a number.
        let lesser = a.lesser(b).to_array();
        assert_eq!(lesser[..3], [0.5, -2.5, 3.0]);
        assert!(lesser[3].is_nan());
        assert_eq!(
            a.where_equal(b, I4::splat(7), I4::splat(1)).to_array(),
            [1, 7, 1, 1]
        );
        assert_eq!(
            b.where_less(a, I4::splat(7), I4::splat(1)).to_array(),
            [7, 1, 1, 1]
        );
        assert_eq!(F4::splat(2.0).to_array(), [2.0; 4]);
    }
}
