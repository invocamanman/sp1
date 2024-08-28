use std::fmt::Debug;

use itertools::Itertools;
use p3_field::Field;
use p3_util::{reverse_bits_len, reverse_slice_index_bits};
use sp1_core_machine::utils::log2_strict_usize;
use sp1_recursion_compiler::ir::Builder;

use crate::{
    hash::{FieldHasher, FieldHasherVariable},
    CircuitConfig,
};

#[derive(Debug, Clone)]
pub struct MerkleTree<F: Field, HV: FieldHasher<F>> {
    /// The height of the tree, not counting the root layer. This is the same as the logarithm of the
    /// number of leaves.
    pub height: usize,

    /// All the layers but the root. If there are `n` leaves where `n` is a power of 2, there are
    /// `2n - 2` elements in this vector. The leaves are at the beginning of the vector.
    pub digest_layers: Vec<HV::Digest>,
}
pub struct VcsError;

impl Debug for VcsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VcsError")
    }
}

impl<F: Field, HV: FieldHasher<F>> MerkleTree<F, HV> {
    pub fn commit(leaves: Vec<HV::Digest>) -> (HV::Digest, Self) {
        assert!(!leaves.is_empty());
        let new_len = leaves.len().next_power_of_two();
        let height = log2_strict_usize(new_len);

        // Pre-allocate the vector.
        let mut digest_layers = Vec::with_capacity(2 * new_len - 2);

        // If `leaves.len()` is not a power of 2, we pad the leaves with default values.
        let mut last_layer = leaves;
        let old_len = last_layer.len();
        for _ in old_len..new_len {
            last_layer.push(HV::Digest::default());
        }

        // Store the leaves in bit-reversed order.
        reverse_slice_index_bits(&mut last_layer);

        digest_layers.extend(last_layer.iter());

        // Compute the rest of the layers.
        for _ in 0..height - 1 {
            let mut next_layer = Vec::with_capacity(last_layer.len() / 2);
            for (a, b) in last_layer.iter().tuples() {
                next_layer.push(HV::constant_compress([*a, *b]));
            }
            digest_layers.extend(next_layer.iter());

            last_layer = next_layer;
        }

        debug_assert_eq!(digest_layers.len(), 2 * new_len - 2);

        let root = HV::constant_compress([last_layer[0], last_layer[1]]);
        (root, Self { height, digest_layers })
    }

    pub fn open(&self, index: usize) -> (HV::Digest, Vec<HV::Digest>) {
        let mut path = Vec::with_capacity(self.height);
        let mut bit_rev_index = reverse_bits_len(index, self.height);
        let value = self.digest_layers[bit_rev_index];

        // Variable to keep track index of the first element in the current layer.
        let mut offset = 0;
        for i in 0..self.height {
            let sibling = if bit_rev_index % 2 == 0 {
                self.digest_layers[offset + bit_rev_index + 1]
            } else {
                self.digest_layers[offset + bit_rev_index - 1]
            };
            path.push(sibling);
            bit_rev_index >>= 1;

            // The current layer has 1 << (height - i) elements, so we shift offset by that amount.
            offset += 1 << (self.height - i);
        }
        debug_assert_eq!(path.len(), self.height);
        (value, path)
    }

    pub fn verify(
        index: usize,
        value: HV::Digest,
        path: &[HV::Digest],
        commitment: HV::Digest,
    ) -> Result<(), VcsError> {
        let mut value = value;

        let mut index = reverse_bits_len(index, path.len());

        for sibling in path {
            let sibling = *sibling;

            // If the index is odd, swap the order of [value, sibling].
            let new_pair = if index % 2 == 0 { [value, sibling] } else { [sibling, value] };
            value = HV::constant_compress(new_pair);
            index >>= 1;
        }
        if value == commitment {
            Ok(())
        } else {
            Err(VcsError)
        }
    }
}

pub fn verify<C: CircuitConfig, HV: FieldHasherVariable<C>>(
    builder: &mut Builder<C>,
    index: Vec<C::Bit>,
    value: HV::DigestVariable,
    path: &[HV::DigestVariable],
    commitment: HV::DigestVariable,
) {
    let mut value = value;
    for (sibling, bit) in path.iter().zip(index.iter().rev()) {
        let sibling = *sibling;

        // If the index is odd, swap the order of [value, sibling].
        let new_pair = HV::select_chain_digest(builder, *bit, [value, sibling]);
        value = HV::compress(builder, new_pair);
    }
    HV::assert_digest_eq(builder, value, commitment);
}

#[cfg(test)]
mod tests {
    use itertools::Itertools;
    use p3_baby_bear::BabyBear;
    use p3_field::AbstractField;
    use rand::rngs::OsRng;
    use sp1_recursion_compiler::{
        config::InnerConfig,
        ir::{Builder, Felt},
    };
    use sp1_recursion_core_v2::DIGEST_SIZE;
    use sp1_stark::baby_bear_poseidon2::BabyBearPoseidon2;
    use zkhash::ark_ff::UniformRand;

    use crate::{
        merkle_tree::{verify, MerkleTree},
        utils::tests::run_test_recursion,
        CircuitConfig,
    };
    type C = InnerConfig;
    type F = BabyBear;
    type HV = BabyBearPoseidon2;

    #[test]
    fn test_merkle_tree_inner() {
        let mut rng = OsRng;
        let mut builder = Builder::<InnerConfig>::default();
        for _ in 0..20 {
            let leaves: Vec<[F; DIGEST_SIZE]> =
                (0..17).map(|_| std::array::from_fn(|_| F::rand(&mut rng))).collect();
            let (root, tree) = MerkleTree::<F, HV>::commit(leaves.to_vec());
            for (i, leaf) in leaves.iter().enumerate() {
                let (value, proof) = MerkleTree::<F, HV>::open(&tree, i);
                assert!(value == *leaf);
                MerkleTree::<F, HV>::verify(i, value, &proof, root).unwrap();
                let (value_variable, proof_variable): ([Felt<_>; 8], Vec<[Felt<_>; 8]>) = (
                    std::array::from_fn(|i| builder.constant(value[i])),
                    proof
                        .iter()
                        .map(|x| std::array::from_fn(|i| builder.constant(x[i])))
                        .collect_vec(),
                );

                let index_var = builder.constant(BabyBear::from_canonical_usize(i));
                let index_bits = C::num2bits(&mut builder, index_var, 5);
                let root_variable: [Felt<_>; 8] =
                    root.iter().map(|x| builder.constant(*x)).collect_vec().try_into().unwrap();

                verify::<InnerConfig, BabyBearPoseidon2>(
                    &mut builder,
                    index_bits,
                    value_variable,
                    &proof_variable,
                    root_variable,
                );
            }
        }

        run_test_recursion(builder.operations, std::iter::empty());
    }
}