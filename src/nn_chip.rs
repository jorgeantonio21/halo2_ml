use std::{fmt::Debug, marker::PhantomData};

use icecream::ic;

use halo2_proofs::{
    arithmetic::FieldExt,
    circuit::{AssignedCell, Chip, Layouter, Value},
    plonk::{
        Advice, Assigned, Column, ConstraintSystem, Error as PlonkError, Expression, Instance,
        Selector,
    },
    poly::Rotation,
};

use crate::nn_ops::{eltwise_ops::EltwiseOp, lookup_ops::DecompTable};

use derive_more::Constructor;

//TODO: move somehwere more appropriate
#[derive(Default)]
pub struct LayerParams<F: FieldExt> {
    pub weights: Vec<F>,
    pub biases: Vec<F>,
}

#[derive(Clone, Debug, Constructor)]
pub struct ForwardLayerConfig<F: FieldExt, const BASE: usize, Elt: EltwiseOp<F, BASE>> {
    pub weights: Vec<Column<Advice>>,
    pub bias: Column<Advice>,
    pub input: Column<Advice>,
    pub output: Column<Advice>,
    pub eltwise_config: Elt,
    // pub eltwise_inter: Vec<Column<Advice>>,
    // pub eltwise_output: Column<Advice>,
    pub dims: [usize; 2],
    pub nn_selector: Selector,
    // pub elt_selector: Selector,
    _marker: PhantomData<F>,
}

///Instructions that allow NN Chip to be used
pub trait NNLayerInstructions<F: FieldExt, const BASE: usize>: Chip<F> {
    type Num;
    type Elt: EltwiseOp<F, BASE>;

    ///Loads inputs from constant
    fn load_input_constant(
        &self,
        layouter: impl Layouter<F>,
        input: &[F],
    ) -> Result<Vec<Self::Num>, PlonkError>;

    ///Loads inputs from advice
    fn load_input_advice(
        &self,
        layouter: impl Layouter<F>,
        input: Vec<Value<F>>,
    ) -> Result<Vec<Self::Num>, PlonkError>;

    ///Loads inputs from instance
    fn load_input_instance(
        &self,
        layouter: impl Layouter<F>,
        instance: Column<Instance>,
        row: usize,
        len: usize,
    ) -> Result<Vec<Self::Num>, PlonkError>;

    ///Adds layers to the model, including constants for weights and biases
    fn add_layers(
        &self,
        layouter: impl Layouter<F>,
        input: Vec<Self::Num>,
        layers: &LayerParams<F>,
    ) -> Result<Vec<Self::Num>, PlonkError>;
}

#[derive(Clone, Debug)]
///Chip to prove `NeuralNet` operations
pub struct ForwardLayerChip<F: FieldExt, const BASE: usize, Elt: EltwiseOp<F, BASE>> {
    config: ForwardLayerConfig<F, BASE, Elt>,
    _marker: PhantomData<(F, Elt)>,
}

impl<F: FieldExt, const BASE: usize, Elt: EltwiseOp<F, BASE>> Chip<F>
    for ForwardLayerChip<F, BASE, Elt>
{
    type Config = ForwardLayerConfig<F, BASE, Elt>;
    type Loaded = ();

    fn config(&self) -> &Self::Config {
        &self.config
    }

    fn loaded(&self) -> &Self::Loaded {
        &()
    }
}

impl<F: FieldExt, const BASE: usize, Elt: EltwiseOp<F, BASE>> ForwardLayerChip<F, BASE, Elt> {
    pub fn construct(config: <Self as Chip<F>>::Config) -> Self {
        Self {
            config,
            _marker: PhantomData,
        }
    }

    pub fn configure(
        meta: &mut ConstraintSystem<F>,
        //layer: ForwardLayerConfig<F, BASE, Elt>,
        input: Column<Advice>,
        weights: Vec<Column<Advice>>,
        bias: Column<Advice>,
        output: Column<Advice>,
        eltwise_inter: Vec<Column<Advice>>,
        eltwise_output: Column<Advice>,
        range_table: DecompTable<F, BASE>,
        dims: [usize; 2],
    ) -> <Self as Chip<F>>::Config {
        let nn_selector = meta.selector();
        meta.create_gate("affine", |meta| {
            let q = meta.query_selector(nn_selector);
            let out_dim = dims[0];
            // We put the negation of the claimed output in the constraint tensor.
            let constraints: Vec<Expression<F>> = (0..out_dim)
                .map(|i| -meta.query_advice(output, Rotation(i as i32)))
                .collect();

            // Now we compute the linear expression,  and add it to constraints
            let constraints: Vec<Expression<F>> = constraints
                .iter()
                .enumerate()
                .map(|item| {
                    let i = item.0;
                    let mut c = item.1.clone();
                    for j in 0..dims[1] {
                        c = c + meta.query_advice(weights[i], Rotation(j as i32))
                            * meta.query_advice(input, Rotation(j as i32));
                    }
                    // add the bias
                    q.clone() * (c + meta.query_advice(bias, Rotation(i as i32)))
                })
                .collect();
            constraints
        });
        //}
        let eltwise_config =
            Elt::configure(meta, output, eltwise_inter, eltwise_output, range_table);
        ForwardLayerConfig {
            input,
            weights,
            bias,
            output,
            eltwise_config,
            dims,
            nn_selector,
            _marker: PhantomData,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Number<F: FieldExt>(pub AssignedCell<F, F>);

impl<F: FieldExt, const BASE: usize, Elt: EltwiseOp<F, BASE>> NNLayerInstructions<F, BASE>
    for ForwardLayerChip<F, BASE, Elt>
{
    type Elt = Elt;
    type Num = Number<F>;

    fn load_input_constant(
        &self,
        mut layouter: impl Layouter<F>,
        input: &[F],
    ) -> Result<Vec<Self::Num>, PlonkError> {
        let config = self.config();

        layouter.assign_region(
            || "load constants to NN",
            |mut region| {
                input
                    .iter()
                    .enumerate()
                    .map(|(i, item)| {
                        region
                            .assign_advice_from_constant(
                                || "NN input from constant",
                                config.input,
                                i,
                                *item,
                            )
                            .map(Number)
                    })
                    .collect()
            },
        )
    }

    fn load_input_advice(
        &self,
        mut layouter: impl Layouter<F>,
        input: Vec<Value<F>>,
    ) -> Result<Vec<Self::Num>, PlonkError> {
        let config = self.config();

        layouter.assign_region(
            || "load constants to NN",
            |mut region| {
                input
                    .iter()
                    .enumerate()
                    .map(|(i, item)| {
                        region
                            .assign_advice(|| "NN input from advice", config.input, i, || *item)
                            .map(Number)
                    })
                    .collect()
            },
        )
    }

    fn load_input_instance(
        &self,
        mut layouter: impl Layouter<F>,
        instance: Column<Instance>,
        starting_row: usize,
        len: usize,
    ) -> Result<Vec<Self::Num>, PlonkError> {
        let config = self.config();

        layouter.assign_region(
            || "load constants to NN",
            |mut region| {
                (0..len)
                    .map(|iii| {
                        region
                            .assign_advice_from_instance(
                                || "NN input from instance",
                                instance,
                                starting_row + iii,
                                config.input,
                                iii,
                            )
                            .map(Number)
                    })
                    .collect()
            },
        )
    }

    fn add_layers(
        &self,
        mut layouter: impl Layouter<F>,
        input: Vec<Self::Num>,
        layer: &LayerParams<F>,
    ) -> Result<Vec<Self::Num>, PlonkError> {
        let config = &self.config;

        layouter.assign_region(
            || "NN Layer",
            |mut region| {
                // let layer_outputs: Result<Vec<_>, PlonkError> =
                let offset = 0;

                config.nn_selector.enable(&mut region, offset).unwrap();

                //assign parameters (weights+biases)
                let (biases, weights): (Vec<_>, Vec<_>) = ({
                    let thing: (Result<Vec<_>, _>, Result<Vec<_>, _>) = (
                        layer
                            .biases
                            .iter()
                            .enumerate()
                            .map(|(index, &bias)| {
                                region.assign_advice(
                                    || "assigning biases".to_string(),
                                    config.bias,
                                    offset + index,
                                    || Value::known(bias),
                                )
                            })
                            .collect(),
                        layer
                            .weights
                            .iter()
                            .enumerate()
                            .map(|(iii, weight)| {
                                region.assign_advice(
                                    || "assigning weights".to_string(),
                                    // row indices
                                    config.weights[iii % config.dims[1]],
                                    // columns indices
                                    offset + (iii / config.dims[1]),
                                    || Value::known(*weight),
                                )
                            })
                            .collect(),
                    );
                    Ok::<_, PlonkError>((thing.0.unwrap(), thing.1.unwrap()))
                })
                .unwrap();

                //calculate output and assign it to layer output
                let mat_output: Vec<AssignedCell<Assigned<F>, F>> = {
                    let out_dim = config.dims[0];

                    for (i, x) in input.iter().enumerate() {
                        x.0.copy_advice(|| "input", &mut region, config.input, offset + i)?;
                    }

                    // calculate value of output
                    let output: Vec<Value<Assigned<F>>> = (0..out_dim)
                        .map(|i| {
                            let mut o: Value<Assigned<F>> = Value::known(F::zero().into());
                            for (j, x) in input.iter().enumerate() {
                                o = o + weights[i + (j * config.dims[0])].value_field()
                                    * x.0.value_field();
                            }
                            let x = o + biases[i].value_field();
                            x
                        })
                        .collect();
                    
                    let output: Vec<AssignedCell<Assigned<F>, F>> = output
                        .iter()
                        .enumerate()
                        .map(|(i, o)| {
                            region
                                .assign_advice(|| "o".to_string(), config.output, offset + i, || *o)
                                .unwrap()
                        })
                        .collect();
                    output
                };

                let eltwise_output: Result<Vec<_>, PlonkError> =
                    config
                        .eltwise_config
                        .layout(&mut region, mat_output, offset);
                eltwise_output
            },
        )
    }
}

//TODO move somewhere
pub fn felt_to_i128<F: FieldExt>(x: F) -> i128 {
    if x > F::TWO_INV {
        -((-x).get_lower_128() as i128)
    } else {
        x.get_lower_128() as i128
    }
}
