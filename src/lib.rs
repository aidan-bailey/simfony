/// Library for parsing and compiling simfony

pub type ProgNode = Arc<named::ConstructNode>;

pub mod array;
pub mod ast;
pub mod compile;
pub mod debug;
pub mod dummy_env;
pub mod error;
pub mod jet;
pub mod named;
pub mod num;
pub mod parse;
pub mod pattern;
pub mod str;
pub mod types;
pub mod value;
pub mod witness;

use std::sync::Arc;

use simplicity::{jet::Elements, CommitNode, RedeemNode};

pub extern crate simplicity;
pub use simplicity::elements;

use crate::ast::DeclaredWitnesses;
use crate::debug::DebugSymbols;
use crate::error::WithFile;
use crate::parse::ParseFromStr;
use crate::witness::WitnessValues;

/// A Simfony program, compiled to Simplicity.
#[derive(Clone, Debug)]
pub struct CompiledProgram {
    simplicity: ProgNode,
    witness_types: DeclaredWitnesses,
    debug_symbols: DebugSymbols,
}

impl CompiledProgram {
    /// Parse and compile a Simfony program from the given string.
    ///
    /// ## Errors
    ///
    /// The string is not a valid Simfony program.
    pub fn new(s: &str) -> Result<Self, String> {
        let parse_program = parse::Program::parse_from_str(s)?;
        let ast_program = ast::Program::analyze(&parse_program).with_file(s)?;
        let simplicity_named_construct = ast_program.compile().with_file(s)?;
        Ok(Self {
            simplicity: simplicity_named_construct,
            witness_types: ast_program.witnesses().clone(),
            debug_symbols: ast_program.debug_symbols(s),
        })
    }

    /// Access the debug symbols for the Simplicity target code.
    pub fn debug_symbols(&self) -> &DebugSymbols {
        &self.debug_symbols
    }

    /// Access the Simplicity target code, without witness data.
    pub fn commit(&self) -> Arc<CommitNode<Elements>> {
        named::to_commit_node(&self.simplicity).expect("Compiled Simfony program has type 1 -> 1")
    }

    /// Satisfy the Simfony program with the given `witness_values`.
    ///
    /// ## Errors
    ///
    /// - Witness values have a different type than declared in the Simfony program.
    /// - There are missing witness values.
    pub fn satisfy(&self, witness_values: &WitnessValues) -> Result<SatisfiedProgram, String> {
        witness_values
            .is_consistent(&self.witness_types)
            .map_err(|e| e.to_string())?;
        let simplicity_witness = named::to_witness_node(&self.simplicity, witness_values);
        let simplicity_redeem = simplicity_witness.finalize().map_err(|e| e.to_string())?;
        Ok(SatisfiedProgram {
            simplicity: simplicity_redeem,
            debug_symbols: self.debug_symbols.clone(),
        })
    }
}

/// A Simfony program, compiled to Simplicity and satisfied with witness data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SatisfiedProgram {
    simplicity: Arc<RedeemNode<Elements>>,
    debug_symbols: DebugSymbols,
}

impl SatisfiedProgram {
    /// Parse, compile and satisfy a Simfony program
    /// from the given string and the given `witness_values`.
    ///
    /// ## See
    ///
    /// - [`CompiledProgram::new`]
    /// - [`CompiledProgram::satisfy`]
    pub fn new(s: &str, witness_values: &WitnessValues) -> Result<Self, String> {
        let compiled = CompiledProgram::new(s)?;
        compiled.satisfy(witness_values)
    }

    /// Access the Simplicity target code, including witness data.
    pub fn redeem(&self) -> &Arc<RedeemNode<Elements>> {
        &self.simplicity
    }

    /// Access the debug symbols for the Simplicity target code.
    pub fn debug_symbols(&self) -> &DebugSymbols {
        &self.debug_symbols
    }
}

/// Recursively implement [`PartialEq`], [`Eq`] and [`std::hash::Hash`]
/// using selected members of a given type. The type must have a getter
/// method for each selected member.
#[macro_export]
macro_rules! impl_eq_hash {
    ($ty: ident; $($member: ident),*) => {
        impl PartialEq for $ty {
            fn eq(&self, other: &Self) -> bool {
                true $(&& self.$member() == other.$member())*
            }
        }

        impl Eq for $ty {}

        impl std::hash::Hash for $ty {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                $(self.$member().hash(state);)*
            }
        }
    };
}

/// Helper trait for implementing [`arbitrary::Arbitrary`] for recursive structures.
///
/// [`ArbitraryRec::arbitrary_rec`] allows the caller to set a budget that is decreased every time
/// the generated structure gets deeper. The maximum depth of the generated structure is equal to
/// the initial budget. The budget prevents the generated structure from becoming too deep, which
/// could cause issues in the code that processes these structures.
///
/// https://github.com/rust-fuzz/arbitrary/issues/78
#[cfg(feature = "arbitrary")]
trait ArbitraryRec: Sized {
    /// Generate a recursive structure from unstructured data.
    ///
    /// Generate leaves or parents when the budget is positive.
    /// Generate only leaves when the budget is zero.
    ///
    /// ## Implementation
    ///
    /// Recursive calls of [`arbitrary_rec`] must decrease the budget by one.
    fn arbitrary_rec(u: &mut arbitrary::Unstructured, budget: usize) -> arbitrary::Result<Self>;
}

/// Helper trait for implementing [`arbitrary::Arbitrary`] for typed structures.
///
/// [`arbitrary::Arbitrary`] is intended to produce well-formed values.
/// Structures with an internal type should be generated in a well-typed fashion.
///
/// [`arbitrary::Arbitrary`] can be implemented for a typed structure as follows:
/// 1. Generate the type via [`arbitrary::Arbitrary`].
/// 2. Generate the structure via [`ArbitraryOfType::arbitrary_of_type`].
#[cfg(feature = "arbitrary")]
trait ArbitraryOfType: Sized {
    /// Internal type of the structure.
    type Type;

    /// Generate a structure of the given type.
    fn arbitrary_of_type(
        u: &mut arbitrary::Unstructured,
        ty: &Self::Type,
    ) -> arbitrary::Result<Self>;
}

#[cfg(test)]
mod tests {
    use base64::display::Base64Display;
    use base64::engine::general_purpose::STANDARD;
    use simplicity::BitMachine;
    use std::borrow::Cow;
    use std::path::Path;

    use crate::*;

    struct TestCase<T> {
        program: T,
        lock_time: elements::LockTime,
        sequence: elements::Sequence,
    }

    impl<'a> TestCase<CompiledProgram> {
        pub fn program_file<P: AsRef<Path>>(program_file_path: P) -> Self {
            let program_text = std::fs::read_to_string(program_file_path).unwrap();
            Self::program_text(Cow::Owned(program_text))
        }

        pub fn program_text(program_text: Cow<'a, str>) -> Self {
            let program = match CompiledProgram::new(program_text.as_ref()) {
                Ok(x) => x,
                Err(error) => panic!("{error}"),
            };
            Self {
                program,
                lock_time: elements::LockTime::ZERO,
                sequence: elements::Sequence::MAX,
            }
        }

        pub fn with_witness_file<P: AsRef<Path>>(
            self,
            witness_file_path: P,
        ) -> TestCase<SatisfiedProgram> {
            let witness_text = std::fs::read_to_string(witness_file_path).unwrap();
            let witness_values = match serde_json::from_str::<WitnessValues>(&witness_text) {
                Ok(x) => x,
                Err(error) => panic!("{error}"),
            };
            self.with_witness_values(&witness_values)
        }

        pub fn with_witness_values(
            self,
            witness_values: &WitnessValues,
        ) -> TestCase<SatisfiedProgram> {
            let program = match self.program.satisfy(witness_values) {
                Ok(x) => x,
                Err(error) => panic!("{error}"),
            };
            TestCase {
                program,
                lock_time: self.lock_time,
                sequence: self.sequence,
            }
        }
    }

    impl<T> TestCase<T> {
        pub fn with_lock_time(mut self, height: u32) -> Self {
            let height = elements::locktime::Height::from_consensus(height).unwrap();
            self.lock_time = elements::LockTime::Blocks(height);
            if self.sequence.is_final() {
                self.sequence = elements::Sequence::ENABLE_LOCKTIME_NO_RBF;
            }
            self
        }

        pub fn with_sequence(mut self, distance: u16) -> Self {
            self.sequence = elements::Sequence::from_height(distance);
            self
        }

        #[allow(dead_code)]
        pub fn print_sighash_all(self) -> Self {
            let env = dummy_env::dummy_with(self.lock_time, self.sequence);
            dbg!(env.c_tx_env().sighash_all());
            self
        }
    }

    impl TestCase<SatisfiedProgram> {
        #[allow(dead_code)]
        pub fn print_encoding(self) -> Self {
            let (program_bytes, witness_bytes) = self.program.redeem().encode_to_vec();
            println!(
                "Program:\n{}",
                Base64Display::new(&program_bytes, &STANDARD)
            );
            println!(
                "Witness:\n{}",
                Base64Display::new(&witness_bytes, &STANDARD)
            );
            self
        }

        fn run(self) -> Result<(), simplicity::bit_machine::ExecutionError> {
            let mut mac = BitMachine::for_program(self.program.redeem());
            let env = dummy_env::dummy_with(self.lock_time, self.sequence);
            mac.exec(self.program.redeem(), &env).map(|_| ())
        }

        pub fn assert_run_success(self) {
            match self.run() {
                Ok(_) => {}
                Err(error) => panic!("Unexpected error: {error}"),
            }
        }
    }

    #[test]
    fn cat() {
        TestCase::program_file("./examples/cat.simf")
            .with_witness_values(&WitnessValues::empty())
            .assert_run_success();
    }

    #[test]
    fn ctv() {
        TestCase::program_file("./examples/ctv.simf")
            .with_witness_values(&WitnessValues::empty())
            .assert_run_success();
    }

    #[test]
    fn escrow_with_delay_timeout() {
        TestCase::program_file("./examples/escrow_with_delay.simf")
            .with_sequence(1000)
            .print_sighash_all()
            .with_witness_file("./examples/escrow_with_delay.timeout.wit")
            .assert_run_success();
    }

    #[test]
    fn hash_loop() {
        TestCase::program_file("./examples/hash_loop.simf")
            .with_witness_values(&WitnessValues::empty())
            .assert_run_success();
    }

    #[test]
    fn hodl_vault() {
        TestCase::program_file("./examples/hodl_vault.simf")
            .with_lock_time(1000)
            .print_sighash_all()
            .with_witness_file("./examples/hodl_vault.wit")
            .assert_run_success();
    }

    #[test]
    fn htlc_complete() {
        TestCase::program_file("./examples/htlc.simf")
            .print_sighash_all()
            .with_witness_file("./examples/htlc.complete.wit")
            .assert_run_success();
    }

    #[test]
    fn last_will_inherit() {
        TestCase::program_file("./examples/last_will.simf")
            .with_sequence(25920)
            .print_sighash_all()
            .with_witness_file("./examples/last_will.inherit.wit")
            .assert_run_success();
    }

    #[test]
    fn p2ms() {
        TestCase::program_file("./examples/p2ms.simf")
            .print_sighash_all()
            .with_witness_file("./examples/p2ms.wit")
            .assert_run_success();
    }

    #[test]
    fn p2pk() {
        TestCase::program_file("./examples/p2pk.simf")
            .print_sighash_all()
            .with_witness_file("./examples/p2pk.wit")
            .assert_run_success();
    }

    #[test]
    fn p2pkh() {
        TestCase::program_file("./examples/p2pkh.simf")
            .print_sighash_all()
            .with_witness_file("./examples/p2pkh.wit")
            .assert_run_success();
    }

    #[test]
    fn presigned_vault_complete() {
        TestCase::program_file("./examples/presigned_vault.simf")
            .with_sequence(1000)
            .print_sighash_all()
            .with_witness_file("./examples/presigned_vault.complete.wit")
            .assert_run_success();
    }

    #[test]
    fn sighash_all_anyprevoutanyscript() {
        TestCase::program_file("./examples/sighash_all_anyprevoutanyscript.simf")
            .with_witness_file("./examples/sighash_all_anyprevoutanyscript.wit")
            .assert_run_success();
    }

    #[test]
    fn sighash_none() {
        TestCase::program_file("./examples/sighash_none.simf")
            .with_witness_file("./examples/sighash_none.wit")
            .assert_run_success();
    }

    #[test]
    fn transfer_with_timeout_transfer() {
        TestCase::program_file("./examples/transfer_with_timeout.simf")
            .print_sighash_all()
            .with_witness_file("./examples/transfer_with_timeout.transfer.wit")
            .assert_run_success();
    }

    #[test]
    fn redefined_variable() {
        let prog_text = r#"fn main() {
    let beefbabe: (u16, u16) = (0xbeef, 0xbabe);
    let beefbabe: u32 = <(u16, u16)>::into(beefbabe);
}
"#;
        TestCase::program_text(Cow::Borrowed(prog_text))
            .with_witness_values(&WitnessValues::empty())
            .assert_run_success();
    }

    #[test]
    fn empty_function_body_nonempty_return() {
        let prog_text = r#"fn my_true() -> bool {
    // function body is empty, although function must return `bool`
}

fn main() {
    jet_verify(my_true());
}
"#;
        match SatisfiedProgram::new(prog_text, &WitnessValues::empty()) {
            Ok(_) => panic!("Accepted faulty program"),
            Err(error) => {
                if !error.contains("Expected expression of type `bool`, found type `()`") {
                    panic!("Unexpected error: {error}")
                }
            }
        }
    }

    #[test]
    fn fuzz_regression_1() {
        CompiledProgram::new("type f=f").unwrap_err();
    }

    #[test]
    #[ignore]
    fn fuzz_slow_unit_1() {
        CompiledProgram::new("fn fnnfn(MMet:(((sssss,((((((sssss,ssssss,ss,((((((sssss,ss,((((((sssss,ssssss,ss,((((((sssss,ssssss,((((((sssss,sssssssss,(((((((sssss,sssssssss,(((((ssss,((((((sssss,sssssssss,(((((((sssss,ssss,((((((sssss,ss,((((((sssss,ssssss,ss,((((((sssss,ssssss,((((((sssss,sssssssss,(((((((sssss,sssssssss,(((((ssss,((((((sssss,sssssssss,(((((((sssss,sssssssssssss,(((((((((((u|(").unwrap_err();
    }
}
