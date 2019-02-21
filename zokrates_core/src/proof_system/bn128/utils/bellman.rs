extern crate rand;

use bellman::groth16::Proof;
use bellman::groth16::{
    create_random_proof, generate_random_parameters, prepare_verifying_key, verify_proof,
    Parameters, VerifyingKey,
};
use bellman::{Circuit, ConstraintSystem, LinearCombination, SynthesisError, Variable};
use ir::{LinComb, Prog, Statement, Witness};
use pairing::bn256::{Bn256, Fr};
use std::collections::BTreeMap;
use zokrates_field::field::{Field, FieldPrime};

use self::rand::*;
use flat_absy::FlatVariable;

#[derive(Clone)]
pub struct Computation<T: Field> {
    program: Prog<T>,
    witness: Option<Witness<T>>,
}

impl<T: Field> Computation<T> {
    pub fn with_witness(program: Prog<T>, witness: Witness<T>) -> Self {
        Computation {
            program,
            witness: Some(witness),
        }
    }

    pub fn without_witness(program: Prog<T>) -> Self {
        Computation {
            program,
            witness: None,
        }
    }
}

fn bellman_combination<CS: ConstraintSystem<Bn256>>(
    l: LinComb<FieldPrime>,
    cs: &mut CS,
    symbols: &mut BTreeMap<FlatVariable, Variable>,
    witness: &mut Witness<FieldPrime>,
) -> LinearCombination<Bn256> {
    l.0.into_iter()
        .map(|(k, v)| {
            (
                Fr::from(v),
                symbols
                    .entry(k)
                    .or_insert_with(|| {
                        match k.is_output() {
                            true => cs.alloc_input(
                                || format!("{}", k),
                                || {
                                    Ok(witness
                                        .0
                                        .remove(&k)
                                        .ok_or(SynthesisError::AssignmentMissing)?
                                        .into())
                                },
                            ),
                            false => cs.alloc(
                                || format!("{}", k),
                                || {
                                    Ok(witness
                                        .0
                                        .remove(&k)
                                        .ok_or(SynthesisError::AssignmentMissing)?
                                        .into())
                                },
                            ),
                        }
                        .unwrap()
                    })
                    .clone(),
            )
        })
        .fold(LinearCombination::zero(), |acc, e| acc + e)
}

impl Prog<FieldPrime> {
    pub fn synthesize<CS: ConstraintSystem<Bn256>>(
        self,
        cs: &mut CS,
        witness: Option<Witness<FieldPrime>>,
    ) -> Result<(), SynthesisError> {
        // mapping from IR variables
        let mut symbols = BTreeMap::new();

        let mut witness = witness.unwrap_or(Witness::empty());

        assert!(symbols.insert(FlatVariable::one(), CS::one()).is_none());

        symbols.extend(
            self.main
                .arguments
                .iter()
                .zip(self.private)
                .enumerate()
                .map(|(index, (var, private))| {
                    let wire = match private {
                        true => cs.alloc(
                            || format!("PRIVATE_INPUT_{}", index),
                            || {
                                Ok(witness
                                    .0
                                    .remove(&var)
                                    .ok_or(SynthesisError::AssignmentMissing)?
                                    .into())
                            },
                        ),
                        false => cs.alloc_input(
                            || format!("PUBLIC_INPUT_{}", index),
                            || {
                                Ok(witness
                                    .0
                                    .remove(&var)
                                    .ok_or(SynthesisError::AssignmentMissing)?
                                    .into())
                            },
                        ),
                    }
                    .unwrap();
                    (var.clone(), wire)
                }),
        );

        let main = self.main;

        for statement in main.statements {
            match statement {
                Statement::Constraint(quad, lin) => {
                    let a = &bellman_combination(quad.left.clone(), cs, &mut symbols, &mut witness);
                    let b =
                        &bellman_combination(quad.right.clone(), cs, &mut symbols, &mut witness);
                    let c = &bellman_combination(lin, cs, &mut symbols, &mut witness);

                    cs.enforce(|| "Constraint", |lc| lc + a, |lc| lc + b, |lc| lc + c);
                }
                _ => {}
            }
        }

        Ok(())
    }
}

impl Computation<FieldPrime> {
    pub fn prove(self, params: &Parameters<Bn256>) -> Proof<Bn256> {
        let rng = &mut thread_rng();
        let proof = create_random_proof(self.clone(), params, rng).unwrap();

        let pvk = prepare_verifying_key(&params.vk);

        // extract public inputs
        let public_inputs = self.public_inputs_values();

        assert!(verify_proof(&pvk, &proof, &public_inputs).unwrap());

        proof
    }

    pub fn public_inputs_values(&self) -> Vec<Fr> {
        self.program
            .main
            .arguments
            .clone()
            .iter()
            .zip(self.program.private.clone())
            .filter(|(_, p)| !p)
            .map(|(a, _)| a)
            .map(|v| self.witness.clone().unwrap().0.get(v).unwrap().clone())
            .chain(self.witness.clone().unwrap().return_values())
            .map(|v| Fr::from(v.clone()))
            .collect()
    }

    pub fn setup(self) -> Parameters<Bn256> {
        let rng = &mut thread_rng();
        // run setup phase
        generate_random_parameters(self, rng).unwrap()
    }
}

impl Circuit<Bn256> for Computation<FieldPrime> {
    fn synthesize<CS: ConstraintSystem<Bn256>>(self, cs: &mut CS) -> Result<(), SynthesisError> {
        self.program.synthesize(cs, self.witness)
    }
}

pub fn serialize_vk(vk: VerifyingKey<Bn256>) -> String {
    format!(
        "vk.alpha = {}
vk.beta = {}
vk.gamma = {}
vk.delta = {}
vk.gammaABC.len() = {}
{}",
        vk.alpha_g1,
        vk.beta_g2,
        vk.gamma_g2,
        vk.delta_g2,
        vk.ic.len(),
        vk.ic
            .iter()
            .enumerate()
            .map(|(i, x)| format!("vk.gammaABC[{}] = {}", i, x))
            .collect::<Vec<_>>()
            .join("\n")
    )
    .replace("G2(x=Fq2(Fq(", "[[")
    .replace("), y=Fq(", ", ")
    .replace("G1(x=Fq(", "[")
    .replace(") + Fq(", ", ")
    .replace("))", "]")
    .replace(") * u), y=Fq2(Fq(", "], [")
    .replace(") * u]", "]]")
}

pub fn serialize_proof(p: &Proof<Bn256>, inputs: &Vec<Fr>) -> String {
    format!(
        "{{
    \"proof\": {{
        \"a\": {},
        \"b\": {},
        \"c\": {}
    }},
    \"inputs\": [{}]
}}",
        p.a,
        p.b,
        p.c,
        inputs
            .iter()
            .map(|v| format!("\"{}\"", v))
            .collect::<Vec<_>>()
            .join(", "),
    )
    .replace("G2(x=Fq2(Fq(", "[[\"")
    .replace("), y=Fq(", "\", \"")
    .replace("G1(x=Fq(", "[\"")
    .replace(") + Fq(", "\", \"")
    .replace(") * u), y=Fq2(Fq(", "\"], [\"")
    .replace(") * u]", "\"]]")
    .replace(") * u))", "\"]]")
    .replace("))", "\"]")
    .replace("Fr(", "")
    .replace(")", "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ir::Function;
    use zokrates_field::field::FieldPrime;

    mod prove {
        use super::*;

        #[test]
        fn empty() {
            let program: Prog<FieldPrime> = Prog {
                main: Function {
                    id: String::from("main"),
                    arguments: vec![],
                    returns: vec![],
                    statements: vec![],
                },
                private: vec![],
            };

            let witness = program.clone().execute::<FieldPrime>(&vec![]).unwrap();
            let computation = Computation::with_witness(program, witness);

            let params = computation.clone().setup();
            let _proof = computation.prove(&params);
        }

        #[test]
        fn identity() {
            let program: Prog<FieldPrime> = Prog {
                main: Function {
                    id: String::from("main"),
                    arguments: vec![FlatVariable::new(0)],
                    returns: vec![FlatVariable::public(0)],
                    statements: vec![Statement::Constraint(
                        FlatVariable::new(0).into(),
                        FlatVariable::public(0).into(),
                    )],
                },
                private: vec![true],
            };

            let witness = program
                .clone()
                .execute::<FieldPrime>(&vec![FieldPrime::from(0)])
                .unwrap();
            let computation = Computation::with_witness(program, witness);

            let params = computation.clone().setup();
            let _proof = computation.prove(&params);
        }

        #[test]
        fn public_identity() {
            let program: Prog<FieldPrime> = Prog {
                main: Function {
                    id: String::from("main"),
                    arguments: vec![FlatVariable::new(0)],
                    returns: vec![FlatVariable::public(0)],
                    statements: vec![Statement::Constraint(
                        FlatVariable::new(0).into(),
                        FlatVariable::public(0).into(),
                    )],
                },
                private: vec![false],
            };

            let witness = program
                .clone()
                .execute::<FieldPrime>(&vec![FieldPrime::from(0)])
                .unwrap();
            let computation = Computation::with_witness(program, witness);

            let params = computation.clone().setup();
            let _proof = computation.prove(&params);
        }

        #[test]
        fn no_arguments() {
            let program: Prog<FieldPrime> = Prog {
                main: Function {
                    id: String::from("main"),
                    arguments: vec![],
                    returns: vec![FlatVariable::public(0)],
                    statements: vec![Statement::Constraint(
                        FlatVariable::one().into(),
                        FlatVariable::public(0).into(),
                    )],
                },
                private: vec![],
            };

            let witness = program.clone().execute::<FieldPrime>(&vec![]).unwrap();
            let computation = Computation::with_witness(program, witness);

            let params = computation.clone().setup();
            let _proof = computation.prove(&params);
        }

        #[test]
        fn unordered_variables() {
            // public variables must be ordered from 0
            // private variables can be unordered
            let program: Prog<FieldPrime> = Prog {
                main: Function {
                    id: String::from("main"),
                    arguments: vec![FlatVariable::new(42), FlatVariable::new(51)],
                    returns: vec![FlatVariable::public(0), FlatVariable::public(1)],
                    statements: vec![
                        Statement::Constraint(
                            (LinComb::from(FlatVariable::new(42))
                                + LinComb::from(FlatVariable::new(51)))
                            .into(),
                            FlatVariable::public(0).into(),
                        ),
                        Statement::Constraint(
                            (LinComb::from(FlatVariable::one())
                                + LinComb::from(FlatVariable::new(42)))
                            .into(),
                            FlatVariable::public(1).into(),
                        ),
                    ],
                },
                private: vec![true, false],
            };

            let witness = program
                .clone()
                .execute::<FieldPrime>(&vec![FieldPrime::from(3), FieldPrime::from(4)])
                .unwrap();
            let computation = Computation::with_witness(program, witness);

            let params = computation.clone().setup();
            let _proof = computation.prove(&params);
        }

        #[test]
        fn one() {
            let program: Prog<FieldPrime> = Prog {
                main: Function {
                    id: String::from("main"),
                    arguments: vec![FlatVariable::new(42)],
                    returns: vec![FlatVariable::public(0)],
                    statements: vec![Statement::Constraint(
                        (LinComb::from(FlatVariable::new(42)) + LinComb::one()).into(),
                        FlatVariable::public(0).into(),
                    )],
                },
                private: vec![false],
            };

            let witness = program
                .clone()
                .execute::<FieldPrime>(&vec![FieldPrime::from(3)])
                .unwrap();
            let computation = Computation::with_witness(program, witness);

            let params = computation.clone().setup();
            let _proof = computation.prove(&params);
        }

        #[test]
        fn with_directives() {
            let program: Prog<FieldPrime> = Prog {
                main: Function {
                    id: String::from("main"),
                    arguments: vec![FlatVariable::new(42), FlatVariable::new(51)],
                    returns: vec![FlatVariable::public(0)],
                    statements: vec![Statement::Constraint(
                        (LinComb::from(FlatVariable::new(42))
                            + LinComb::from(FlatVariable::new(51)))
                        .into(),
                        FlatVariable::public(0).into(),
                    )],
                },
                private: vec![true, false],
            };

            let witness = program
                .clone()
                .execute::<FieldPrime>(&vec![FieldPrime::from(3), FieldPrime::from(4)])
                .unwrap();
            let computation = Computation::with_witness(program, witness);

            let params = computation.clone().setup();
            let _proof = computation.prove(&params);
        }
    }

    mod serialize {
        use super::*;

        mod proof {
            use super::*;

            #[allow(dead_code)]
            #[derive(Deserialize)]
            struct G16ProofPoints {
                a: [String; 2],
                b: [[String; 2]; 2],
                c: [String; 2],
            }

            #[allow(dead_code)]
            #[derive(Deserialize)]
            struct G16Proof {
                proof: G16ProofPoints,
                inputs: Vec<String>,
            }

            #[test]
            fn serialize() {
                let program: Prog<FieldPrime> = Prog {
                    main: Function {
                        id: String::from("main"),
                        arguments: vec![FlatVariable::new(0)],
                        returns: vec![FlatVariable::public(0)],
                        statements: vec![Statement::Constraint(
                            FlatVariable::new(0).into(),
                            FlatVariable::public(0).into(),
                        )],
                    },
                    private: vec![false],
                };

                let witness = program
                    .clone()
                    .execute::<FieldPrime>(&vec![FieldPrime::from(42)])
                    .unwrap();
                let computation = Computation::with_witness(program, witness);

                let public_inputs_values = computation.public_inputs_values();

                let params = computation.clone().setup();
                let proof = computation.prove(&params);

                let serialized_proof = serialize_proof(&proof, &public_inputs_values);
                serde_json::from_str::<G16Proof>(&serialized_proof).unwrap();
            }
        }
    }
}
