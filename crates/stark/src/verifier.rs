use core::fmt::Display;
use std::{
    fmt::{Debug, Formatter},
    marker::PhantomData,
};

use itertools::Itertools;
use p3_air::{Air, BaseAir};
use p3_challenger::{CanObserve, FieldChallenger};
use p3_commit::{LagrangeSelectors, Pcs, PolynomialSpace};
use p3_field::{AbstractExtensionField, AbstractField};

use super::{
    folder::VerifierConstraintFolder,
    types::{AirOpenedValues, ChipOpenedValues, ShardCommitment, ShardProof},
    Domain, OpeningError, StarkGenericConfig, StarkVerifyingKey, Val,
};
use crate::{air::MachineAir, MachineChip};

/// A verifier for a collection of air chips.
pub struct Verifier<SC, A>(PhantomData<SC>, PhantomData<A>);

impl<SC: StarkGenericConfig, A: MachineAir<Val<SC>>> Verifier<SC, A> {
    /// Verify a proof for a collection of air chips.
    #[allow(clippy::too_many_lines)]
    pub fn verify_shard(
        config: &SC,
        vk: &StarkVerifyingKey<SC>,
        chips: &[&MachineChip<SC, A>],
        challenger: &mut SC::Challenger,
        proof: &ShardProof<SC>,
    ) -> Result<(), VerificationError<SC>>
    where
        A: for<'a> Air<VerifierConstraintFolder<'a, SC>>,
    {
        use itertools::izip;

        let ShardProof {
            commitment,
            opened_values,
            opening_proof,
            chip_ordering,
            public_values,
            ..
        } = proof;

        let pcs = config.pcs();

        if chips.len() != opened_values.chips.len() {
            return Err(VerificationError::ChipOpeningLengthMismatch);
        }

        let log_degrees = opened_values.chips.iter().map(|val| val.log_degree).collect::<Vec<_>>();

        let log_quotient_degrees =
            chips.iter().map(|chip| chip.log_quotient_degree()).collect::<Vec<_>>();

        let trace_domains = log_degrees
            .iter()
            .map(|log_degree| pcs.natural_domain_for_degree(1 << log_degree))
            .collect::<Vec<_>>();

        let ShardCommitment { main_commit, permutation_commit, quotient_commit } = commitment;

        let permutation_challenges =
            (0..2).map(|_| challenger.sample_ext_element::<SC::Challenge>()).collect::<Vec<_>>();

        challenger.observe(permutation_commit.clone());

        let alpha = challenger.sample_ext_element::<SC::Challenge>();

        // Observe the quotient commitments.
        challenger.observe(quotient_commit.clone());

        let zeta = challenger.sample_ext_element::<SC::Challenge>();

        let preprocessed_domains_points_and_opens = vk
            .chip_information
            .iter()
            .map(|(name, domain, _)| {
                let i = chip_ordering[name];
                let values = opened_values.chips[i].preprocessed.clone();
                (
                    *domain,
                    vec![(zeta, values.local), (domain.next_point(zeta).unwrap(), values.next)],
                )
            })
            .collect::<Vec<_>>();

        let main_domains_points_and_opens = trace_domains
            .iter()
            .zip_eq(opened_values.chips.iter())
            .map(|(domain, values)| {
                (
                    *domain,
                    vec![
                        (zeta, values.main.local.clone()),
                        (domain.next_point(zeta).unwrap(), values.main.next.clone()),
                    ],
                )
            })
            .collect::<Vec<_>>();

        let perm_domains_points_and_opens = trace_domains
            .iter()
            .zip_eq(opened_values.chips.iter())
            .map(|(domain, values)| {
                (
                    *domain,
                    vec![
                        (zeta, values.permutation.local.clone()),
                        (domain.next_point(zeta).unwrap(), values.permutation.next.clone()),
                    ],
                )
            })
            .collect::<Vec<_>>();

        let quotient_chunk_domains = trace_domains
            .iter()
            .zip_eq(log_degrees)
            .zip_eq(log_quotient_degrees)
            .map(|((domain, log_degree), log_quotient_degree)| {
                let quotient_degree = 1 << log_quotient_degree;
                let quotient_domain =
                    domain.create_disjoint_domain(1 << (log_degree + log_quotient_degree));
                quotient_domain.split_domains(quotient_degree)
            })
            .collect::<Vec<_>>();

        let quotient_domains_points_and_opens = proof
            .opened_values
            .chips
            .iter()
            .zip_eq(quotient_chunk_domains.iter())
            .flat_map(|(values, qc_domains)| {
                values
                    .quotient
                    .iter()
                    .zip_eq(qc_domains)
                    .map(move |(values, q_domain)| (*q_domain, vec![(zeta, values.clone())]))
            })
            .collect::<Vec<_>>();

        config
            .pcs()
            .verify(
                vec![
                    (vk.commit.clone(), preprocessed_domains_points_and_opens),
                    (main_commit.clone(), main_domains_points_and_opens),
                    (permutation_commit.clone(), perm_domains_points_and_opens),
                    (quotient_commit.clone(), quotient_domains_points_and_opens),
                ],
                opening_proof,
                challenger,
            )
            .map_err(|e| VerificationError::InvalidopeningArgument(e))?;

        // Verify the constrtaint evaluations.
        for (chip, trace_domain, qc_domains, values) in
            izip!(chips.iter(), trace_domains, quotient_chunk_domains, opened_values.chips.iter(),)
        {
            // Verify the shape of the opening arguments matches the expected values.
            Self::verify_opening_shape(chip, values)
                .map_err(|e| VerificationError::OpeningShapeError(chip.name(), e))?;
            // Verify the constraint evaluation.
            Self::verify_constraints(
                chip,
                values,
                trace_domain,
                qc_domains,
                zeta,
                alpha,
                &permutation_challenges,
                public_values,
            )
            .map_err(|_| VerificationError::OodEvaluationMismatch(chip.name()))?;
        }

        Ok(())
    }

    fn verify_opening_shape(
        chip: &MachineChip<SC, A>,
        opening: &ChipOpenedValues<SC::Challenge>,
    ) -> Result<(), OpeningShapeError> {
        // Verify that the preprocessed width matches the expected value for the chip.
        if opening.preprocessed.local.len() != chip.preprocessed_width() {
            return Err(OpeningShapeError::PreprocessedWidthMismatch(
                chip.preprocessed_width(),
                opening.preprocessed.local.len(),
            ));
        }
        if opening.preprocessed.next.len() != chip.preprocessed_width() {
            return Err(OpeningShapeError::PreprocessedWidthMismatch(
                chip.preprocessed_width(),
                opening.preprocessed.next.len(),
            ));
        }

        // Verify that the main width matches the expected value for the chip.
        if opening.main.local.len() != chip.width() {
            return Err(OpeningShapeError::MainWidthMismatch(
                chip.width(),
                opening.main.local.len(),
            ));
        }
        if opening.main.next.len() != chip.width() {
            return Err(OpeningShapeError::MainWidthMismatch(chip.width(), opening.main.next.len()));
        }

        // Verify that the permutation width matches the expected value for the chip.
        if opening.permutation.local.len() != chip.permutation_width() * SC::Challenge::D {
            return Err(OpeningShapeError::PermutationWidthMismatch(
                chip.permutation_width(),
                opening.permutation.local.len(),
            ));
        }
        if opening.permutation.next.len() != chip.permutation_width() * SC::Challenge::D {
            return Err(OpeningShapeError::PermutationWidthMismatch(
                chip.permutation_width(),
                opening.permutation.next.len(),
            ));
        }

        // Verift that the number of quotient chunks matches the expected value for the chip.
        if opening.quotient.len() != chip.quotient_width() {
            return Err(OpeningShapeError::QuotientWidthMismatch(
                chip.quotient_width(),
                opening.quotient.len(),
            ));
        }
        // For each quotient chunk, verify that the number of elements is equal to the degree of the
        // challenge extension field over the value field.
        for slice in &opening.quotient {
            if slice.len() != SC::Challenge::D {
                return Err(OpeningShapeError::QuotientChunkSizeMismatch(
                    SC::Challenge::D,
                    slice.len(),
                ));
            }
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::needless_pass_by_value)]
    fn verify_constraints(
        chip: &MachineChip<SC, A>,
        opening: &ChipOpenedValues<SC::Challenge>,
        trace_domain: Domain<SC>,
        qc_domains: Vec<Domain<SC>>,
        zeta: SC::Challenge,
        alpha: SC::Challenge,
        permutation_challenges: &[SC::Challenge],
        public_values: &[Val<SC>],
    ) -> Result<(), OodEvaluationMismatch>
    where
        A: for<'a> Air<VerifierConstraintFolder<'a, SC>>,
    {
        let sels = trace_domain.selectors_at_point(zeta);

        // Recompute the quotient at zeta from the chunks.
        let quotient = Self::recompute_quotient(opening, &qc_domains, zeta);
        // Calculate the evaluations of the constraints at zeta.
        let folded_constraints = Self::eval_constraints(
            chip,
            opening,
            &sels,
            alpha,
            permutation_challenges,
            public_values,
        );

        // Check that the constraints match the quotient, i.e.
        //     folded_constraints(zeta) / Z_H(zeta) = quotient(zeta)
        if folded_constraints * sels.inv_zeroifier == quotient {
            Ok(())
        } else {
            Err(OodEvaluationMismatch)
        }
    }

    /// Evaluates the constraints for a chip and opening.
    pub fn eval_constraints(
        chip: &MachineChip<SC, A>,
        opening: &ChipOpenedValues<SC::Challenge>,
        selectors: &LagrangeSelectors<SC::Challenge>,
        alpha: SC::Challenge,
        permutation_challenges: &[SC::Challenge],
        public_values: &[Val<SC>],
    ) -> SC::Challenge
    where
        A: for<'a> Air<VerifierConstraintFolder<'a, SC>>,
    {
        // Reconstruct the prmutation opening values as extention elements.
        let unflatten = |v: &[SC::Challenge]| {
            v.chunks_exact(SC::Challenge::D)
                .map(|chunk| {
                    chunk.iter().enumerate().map(|(e_i, &x)| SC::Challenge::monomial(e_i) * x).sum()
                })
                .collect::<Vec<SC::Challenge>>()
        };

        let perm_opening = AirOpenedValues {
            local: unflatten(&opening.permutation.local),
            next: unflatten(&opening.permutation.next),
        };

        let mut folder = VerifierConstraintFolder::<SC> {
            preprocessed: opening.preprocessed.view(),
            main: opening.main.view(),
            perm: perm_opening.view(),
            perm_challenges: permutation_challenges,
            cumulative_sum: opening.cumulative_sum,
            is_first_row: selectors.is_first_row,
            is_last_row: selectors.is_last_row,
            is_transition: selectors.is_transition,
            alpha,
            accumulator: SC::Challenge::zero(),
            public_values,
            _marker: PhantomData,
        };

        chip.eval(&mut folder);

        folder.accumulator
    }

    /// Recomputes the quotient for a chip and opening.
    pub fn recompute_quotient(
        opening: &ChipOpenedValues<SC::Challenge>,
        qc_domains: &[Domain<SC>],
        zeta: SC::Challenge,
    ) -> SC::Challenge {
        use p3_field::Field;

        let zps = qc_domains
            .iter()
            .enumerate()
            .map(|(i, domain)| {
                qc_domains
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, other_domain)| {
                        other_domain.zp_at_point(zeta) *
                            other_domain.zp_at_point(domain.first_point()).inverse()
                    })
                    .product::<SC::Challenge>()
            })
            .collect_vec();

        opening
            .quotient
            .iter()
            .enumerate()
            .map(|(ch_i, ch)| {
                assert_eq!(ch.len(), SC::Challenge::D);
                ch.iter()
                    .enumerate()
                    .map(|(e_i, &c)| zps[ch_i] * SC::Challenge::monomial(e_i) * c)
                    .sum::<SC::Challenge>()
            })
            .sum::<SC::Challenge>()
    }
}

/// An error that occurs when the openings do not match the expected shape.
pub struct OodEvaluationMismatch;

/// An error that occurs when the shape of the openings does not match the expected shape.
pub enum OpeningShapeError {
    /// The width of the preprocessed trace does not match the expected width.
    PreprocessedWidthMismatch(usize, usize),
    /// The width of the main trace does not match the expected width.
    MainWidthMismatch(usize, usize),
    /// The width of the permutation trace does not match the expected width.
    PermutationWidthMismatch(usize, usize),
    /// The width of the quotient trace does not match the expected width.
    QuotientWidthMismatch(usize, usize),
    /// The chunk size of the quotient trace does not match the expected chunk size.
    QuotientChunkSizeMismatch(usize, usize),
}

/// An error that occurs during the verification.
pub enum VerificationError<SC: StarkGenericConfig> {
    /// opening proof is invalid.
    InvalidopeningArgument(OpeningError<SC>),
    /// Out-of-domain evaluation mismatch.
    ///
    /// `constraints(zeta)` did not match `quotient(zeta) Z_H(zeta)`.
    OodEvaluationMismatch(String),
    /// The shape of the opening arguments is invalid.
    OpeningShapeError(String, OpeningShapeError),
    /// The cpu chip is missing.
    MissingCpuChip,
    /// The length of the chip opening does not match the expected length.
    ChipOpeningLengthMismatch,
}

impl Debug for OpeningShapeError {
    #[allow(clippy::uninlined_format_args)]
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            OpeningShapeError::PreprocessedWidthMismatch(expected, actual) => {
                write!(f, "Preprocessed width mismatch: expected {}, got {}", expected, actual)
            }
            OpeningShapeError::MainWidthMismatch(expected, actual) => {
                write!(f, "Main width mismatch: expected {}, got {}", expected, actual)
            }
            OpeningShapeError::PermutationWidthMismatch(expected, actual) => {
                write!(f, "Permutation width mismatch: expected {}, got {}", expected, actual)
            }
            OpeningShapeError::QuotientWidthMismatch(expected, actual) => {
                write!(f, "Quotient width mismatch: expected {}, got {}", expected, actual)
            }
            OpeningShapeError::QuotientChunkSizeMismatch(expected, actual) => {
                write!(f, "Quotient chunk size mismatch: expected {}, got {}", expected, actual)
            }
        }
    }
}

impl Display for OpeningShapeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl<SC: StarkGenericConfig> Debug for VerificationError<SC> {
    #[allow(clippy::uninlined_format_args)]
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            VerificationError::InvalidopeningArgument(e) => {
                write!(f, "Invalid opening argument: {:?}", e)
            }
            VerificationError::OodEvaluationMismatch(chip) => {
                write!(f, "Out-of-domain evaluation mismatch on chip {}", chip)
            }
            VerificationError::OpeningShapeError(chip, e) => {
                write!(f, "Invalid opening shape for chip {}: {:?}", chip, e)
            }
            VerificationError::MissingCpuChip => {
                write!(f, "Missing CPU chip")
            }
            VerificationError::ChipOpeningLengthMismatch => {
                write!(f, "Chip opening length mismatch")
            }
        }
    }
}

impl<SC: StarkGenericConfig> Display for VerificationError<SC> {
    #[allow(clippy::uninlined_format_args)]
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            VerificationError::InvalidopeningArgument(_) => {
                write!(f, "Invalid opening argument")
            }
            VerificationError::OodEvaluationMismatch(chip) => {
                write!(f, "Out-of-domain evaluation mismatch on chip {}", chip)
            }
            VerificationError::OpeningShapeError(chip, e) => {
                write!(f, "Invalid opening shape for chip {}: {}", chip, e)
            }
            VerificationError::MissingCpuChip => {
                write!(f, "Missing CPU chip in shard")
            }
            VerificationError::ChipOpeningLengthMismatch => {
                write!(f, "Chip opening length mismatch")
            }
        }
    }
}

impl<SC: StarkGenericConfig> std::error::Error for VerificationError<SC> {}
