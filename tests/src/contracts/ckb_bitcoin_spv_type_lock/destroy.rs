use ckb_bitcoin_spv_verifier::types::{core, packed, prelude::Pack as VPack};
use ckb_testtool::{
    ckb_script::{ScriptError, TransactionScriptError},
    ckb_types::{
        bytes::Bytes,
        core::{TransactionBuilder, TransactionView},
        packed::*,
        prelude::*,
    },
    context::Context,
};

use crate::{prelude::*, utilities, Loader};

#[test]
fn normal_case_1() {
    let case = NormalCase { clients_count: 3 };
    test_normal(case);
}

#[test]
fn normal_case_2() {
    let case = NormalCase { clients_count: 5 };
    test_normal(case);
}

#[test]
fn normal_case_3() {
    let case = NormalCase { clients_count: 10 };
    test_normal(case);
}

#[test]
fn normal_case_4() {
    let case = NormalCase { clients_count: 20 };
    test_normal(case);
}

#[test]
fn normal_case_5() {
    let case = NormalCase { clients_count: 30 };
    test_normal(case);
}

struct NormalCase {
    clients_count: u8,
}

fn test_normal(case: NormalCase) {
    with_transaction(case, |context, tx| {
        let _ = context.should_be_passed(tx, MAX_CYCLES);
    });
}

// Reuse the existing valid transaction for focused single-field mutations.
fn with_transaction(case: NormalCase, mut check: impl FnMut(&Context, &TransactionView)) {
    utilities::setup();

    let loader = Loader::default();
    let mut context = Context::default();

    let lock_script = {
        let bin = loader.load_binary("can-update-without-ownership-lock");
        let out_point = context.deploy_cell(bin);
        context
            .build_script(&out_point, Default::default())
            .expect("lock script")
            .as_builder()
            .args([0u8, 1, 2, 3].pack())
            .build()
    };

    let cells_count = usize::from(case.clients_count) + 1;
    let capacity = SPV_CELL_CAP * (u64::from(case.clients_count) + 1);

    let type_script = {
        let original_input = {
            let output = CellOutput::new_builder()
                .capacity(capacity.pack())
                .lock(lock_script.clone())
                .build();
            let out_point = context.create_cell(output, Bytes::new());
            CellInput::new_builder().previous_output(out_point).build()
        };

        let type_id_array = utilities::calculate_type_id(original_input, cells_count);
        let type_id = core::Hash::from_bytes_ref(&type_id_array);
        let args = packed::SpvTypeArgs::new_builder()
            .type_id(type_id.pack())
            .clients_count(case.clients_count.into())
            .build();
        let bin = loader.load_binary("ckb-bitcoin-spv-type-lock");
        let out_point = context.deploy_cell(bin);
        context
            .build_script(&out_point, Default::default())
            .expect("type script")
            .as_builder()
            .args(args.as_slice().pack())
            .build()
    };

    let inputs = {
        let original_outputs = {
            let spv_cell = CellOutput::new_builder()
                .capacity(SPV_CELL_CAP.pack())
                .lock(lock_script.clone())
                .type_(Some(type_script.clone()).pack())
                .build();
            vec![spv_cell.clone(); usize::from(case.clients_count) + 1]
        };
        original_outputs
            .into_iter()
            .map(|output| {
                let out_point = context.create_cell(output, Bytes::new());
                CellInput::new_builder().previous_output(out_point).build()
            })
            .collect::<Vec<_>>()
    };

    let output = {
        CellOutput::new_builder()
            .capacity(capacity.pack())
            .lock(lock_script.clone())
            .build()
    };

    let tx = TransactionBuilder::default()
        .inputs(inputs)
        .output(output)
        .output_data(Vec::<u8>::default().pack())
        .build();
    let tx = context.complete_tx(tx);

    check(&context, &tx);
}

fn assert_type_error(context: &Context, tx: &TransactionView, code: i8) {
    let (cell, _) = context
        .get_cell(&tx.inputs().get(0).unwrap().previous_output())
        .unwrap();
    let script = cell.type_().to_opt().unwrap();
    let expected = ScriptError::validation_failure(&script, code).input_type_script(0);
    let actual = context
        .verify_tx(tx, MAX_CYCLES)
        .expect_err("mutated state transition must fail");
    assert_eq!(
        actual.root_cause().downcast_ref::<TransactionScriptError>(),
        Some(&expected)
    );
}

#[test]
fn rejects_partial_instance_destruction() {
    with_transaction(NormalCase { clients_count: 3 }, |context, tx| {
        context
            .verify_tx(tx, MAX_CYCLES)
            .expect("complete destruction succeeds");
        let inputs: Vec<_> = tx.inputs().into_iter().take(3).collect();
        assert_type_error(
            context,
            &tx.as_advanced_builder().set_inputs(inputs).build(),
            0x2f, // DestroyNotEnoughCells
        );
    });
}
