use std::{cmp::Ordering, mem};

use ckb_bitcoin_spv_prover::DummyService;
use ckb_bitcoin_spv_verifier::types::{core, packed, prelude::Pack as VPack};
use ckb_testtool::{
    ckb_script::{ScriptError, TransactionScriptError},
    ckb_types::{
        bytes::Bytes,
        core::{DepType, TransactionBuilder, TransactionView},
        packed::*,
        prelude::*,
    },
    context::Context,
};

use crate::{prelude::*, utilities, Loader};

#[test]
fn normal_case_1() {
    let case = NormalCase {
        headers_path: "case-0822528_0830592",
        height: 828576,
        clients_count: 3,
        headers_group_size: 1,
    };
    test_normal(case);
}

#[test]
fn normal_case_2() {
    let case = NormalCase {
        headers_path: "case-0822528_0830592",
        height: 826560,
        clients_count: 5,
        headers_group_size: 2,
    };
    test_normal(case);
}

#[test]
fn normal_case_3() {
    let case = NormalCase {
        headers_path: "case-0822528_0830592",
        height: 824544,
        clients_count: 10,
        headers_group_size: 5,
    };
    test_normal(case);
}

#[test]
fn normal_case_4() {
    let case = NormalCase {
        headers_path: "case-0822528_0830592",
        height: 822528,
        clients_count: 20,
        headers_group_size: 10,
    };
    test_normal(case);
}

struct NormalCase<'a> {
    headers_path: &'a str,
    height: u32,
    clients_count: u8,
    headers_group_size: usize,
}

fn test_normal(case: NormalCase) {
    with_transaction(case, |context, tx| {
        let _ = context.should_be_passed(tx, MAX_CYCLES);
        false
    });
}

// Returning true stops after one valid transition for focused mutations.
fn with_transaction(case: NormalCase, mut check: impl FnMut(&Context, &TransactionView) -> bool) {
    utilities::setup();

    let mut header_bins_iter = {
        let headers_path = format!("main-chain/headers/continuous/{}", case.headers_path);
        utilities::find_bin_files(&headers_path, "").into_iter()
    };

    let mut service = {
        let header = loop {
            let header_bin = header_bins_iter.next().unwrap();
            let height: u32 = header_bin
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .parse()
                .unwrap();
            match height.cmp(&case.height) {
                Ordering::Equal => {
                    let header: core::Header =
                        utilities::decode_from_bin_file(&header_bin).unwrap();
                    break header;
                }
                Ordering::Greater => {
                    panic!("not enough headers");
                }
                Ordering::Less => {}
            }
        };

        DummyService::bootstrap(case.height, header).unwrap()
    };

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

    let type_script = {
        let cells_count = usize::from(case.clients_count) + 1;
        let capacity = SPV_CELL_CAP * (u64::from(case.clients_count) + 1);
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

    let mut tip_client_id: u8 = 0;
    let mut headers = Vec::new();
    let mut checked_transitions = 0;
    for header_bin in header_bins_iter {
        let header: core::Header = utilities::decode_from_bin_file(&header_bin).unwrap();
        let height: u32 = header_bin
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        log::trace!("process header-{height} from file {}", header_bin.display());

        headers.push(header);
        if headers.len() < case.headers_group_size {
            continue;
        }

        let input_spv_info = {
            let spv_info = packed::SpvInfo::new_builder()
                .tip_client_id(tip_client_id.into())
                .build();
            let output = CellOutput::new_builder()
                .capacity(SPV_CELL_CAP.pack())
                .lock(lock_script.clone())
                .type_(Some(type_script.clone()).pack())
                .build();
            let out_point = context.create_cell(output, spv_info.as_bytes());
            CellInput::new_builder().previous_output(out_point).build()
        };
        let cell_dep_spv_client = {
            let mut tip_spv_client = service.tip_client();
            tip_spv_client.id = tip_client_id;
            let spv_client: packed::SpvClient = tip_spv_client.pack();
            let output = CellOutput::new_builder()
                .capacity(SPV_CELL_CAP.pack())
                .lock(lock_script.clone())
                .type_(Some(type_script.clone()).pack())
                .build();
            let out_point = context.create_cell(output, spv_client.as_bytes());
            CellDep::new_builder()
                .out_point(out_point)
                .dep_type(DepType::Code.into())
                .build()
        };

        tip_client_id = utilities::next_client_id(tip_client_id, case.clients_count);

        let input_spv_client = {
            let spv_client = packed::SpvClient::new_builder()
                .id(tip_client_id.into())
                .build();
            let output = CellOutput::new_builder()
                .capacity(SPV_CELL_CAP.pack())
                .lock(lock_script.clone())
                .type_(Some(type_script.clone()).pack())
                .build();
            let out_point = context.create_cell(output, spv_client.as_bytes());
            CellInput::new_builder().previous_output(out_point).build()
        };

        let update = service.update(mem::take(&mut headers)).unwrap();

        let witness_spv_client = {
            let type_args = BytesOpt::new_builder()
                .set(Some(Pack::pack(update.as_slice())))
                .build();
            let witness_args = WitnessArgs::new_builder().output_type(type_args).build();
            witness_args.as_bytes()
        };

        let outputs = {
            let output = CellOutput::new_builder()
                .capacity(SPV_CELL_CAP.pack())
                .lock(lock_script.clone())
                .type_(Some(type_script.clone()).pack())
                .build();
            vec![output.clone(); 2]
        };

        let output_spv_info = packed::SpvInfo::new_builder()
            .tip_client_id(tip_client_id.into())
            .build();
        let output_spv_client: packed::SpvClient = {
            let mut tip_spv_client = service.tip_client();
            tip_spv_client.id = tip_client_id;
            tip_spv_client.pack()
        };

        let tx = TransactionBuilder::default()
            .cell_dep(cell_dep_spv_client)
            .inputs(vec![input_spv_info, input_spv_client])
            .outputs(outputs)
            .outputs_data([output_spv_info.as_bytes(), output_spv_client.as_bytes()].pack())
            .witness(Pack::pack(&witness_spv_client))
            .build();
        let tx = context.complete_tx(tx);

        checked_transitions += 1;
        if check(&context, &tx) {
            break;
        }
    }
    assert!(
        checked_transitions > 0,
        "fixture must produce at least one checked update transition"
    );
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
fn rejects_invalid_update_state_and_proof_envelope() {
    with_transaction(
        NormalCase {
            headers_path: "case-0822528_0830592",
            height: 828576,
            clients_count: 3,
            headers_group_size: 1,
        },
        |context, tx| {
            context
                .verify_tx(tx, MAX_CYCLES)
                .expect("unmodified update succeeds");
            let mut data: Vec<_> = tx.outputs_data().into_iter().collect();
            data[0] = packed::SpvInfo::new_builder().build().as_slice().pack();
            assert_type_error(
                context,
                &tx.as_advanced_builder().set_outputs_data(data).build(),
                0x35, // UpdateOutputInfoChanged
            );
            let deps: Vec<_> = tx.cell_deps().into_iter().skip(1).collect();
            assert_type_error(
                context,
                &tx.as_advanced_builder().set_cell_deps(deps).build(),
                0x37, // UpdateCellDepNotFound
            );
            let witness = WitnessArgs::new_builder().build().as_bytes();
            assert_type_error(
                context,
                &tx.as_advanced_builder()
                    .set_witnesses(vec![Pack::pack(&witness)])
                    .build(),
                0x3a, // UpdateWitnessIsNotExisted
            );
            let witness = WitnessArgs::new_builder()
                .output_type(Some(Bytes::from_static(b"bad")).pack())
                .build()
                .as_bytes();
            assert_type_error(
                context,
                &tx.as_advanced_builder()
                    .set_witnesses(vec![Pack::pack(&witness)])
                    .build(),
                0x04, // Encoding
            );
            let mut data: Vec<_> = tx.outputs_data().into_iter().collect();
            data[1] = Pack::pack(&Bytes::from_static(b"bad client"));
            assert_type_error(
                context,
                &tx.as_advanced_builder().set_outputs_data(data).build(),
                0x34, // UpdateOutputClientNotFound
            );
            let mut data: Vec<_> = tx.outputs_data().into_iter().collect();
            let client = packed::SpvClient::from_slice(&data[1].raw_data()).unwrap();
            data[1] = client
                .as_builder()
                .tip_block_hash(Default::default())
                .build()
                .as_slice()
                .pack();
            assert_type_error(
                context,
                &tx.as_advanced_builder().set_outputs_data(data).build(),
                0x60 + ckb_bitcoin_spv_verifier::error::UpdateError::ClientTipBlockHash as i8,
            );
            true
        },
    );
}
