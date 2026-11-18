use ckb_bitcoin_spv_prover::DummyService;
use ckb_bitcoin_spv_verifier::types::{core, packed, prelude::Pack as VPack};
use ckb_testtool::{
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
    let case = NormalCase {
        headers_path: "case-0822528_0830592",
        height: 822528,
        clients_count: 3,
    };
    test_normal(case);
}

#[test]
fn normal_case_2() {
    let case = NormalCase {
        headers_path: "case-0822528_0830592",
        height: 824544,
        clients_count: 5,
    };
    test_normal(case);
}

#[test]
fn normal_case_3() {
    let case = NormalCase {
        headers_path: "case-0822528_0830592",
        height: 826560,
        clients_count: 10,
    };
    test_normal(case);
}

#[test]
fn normal_case_4() {
    let case = NormalCase {
        headers_path: "case-0822528_0830592",
        height: 828576,
        clients_count: 20,
    };
    test_normal(case);
}

#[test]
fn normal_case_5() {
    let case = NormalCase {
        headers_path: "case-0822528_0830592",
        height: 830592,
        clients_count: 30,
    };
    test_normal(case);
}

struct NormalCase<'a> {
    headers_path: &'a str,
    height: u32,
    clients_count: u8,
}

fn test_normal(case: NormalCase) {
    let (context, tx) = build_create(case);
    let _ = context.should_be_passed(&tx, MAX_CYCLES);
}

fn build_create(case: NormalCase) -> (Context, TransactionView) {
    utilities::setup();

    let (service, bootstrap) = {
        let headers_path = format!("main-chain/headers/continuous/{}", case.headers_path);
        let filename = format!("{:07}.bin", case.height);

        let header_bin = utilities::find_bin_file(&headers_path, &filename);
        let header: core::Header = utilities::decode_from_bin_file(&header_bin).unwrap();

        log::trace!(
            "process header-{} from file {}",
            case.height,
            header_bin.display()
        );

        let bootstrap = packed::SpvBootstrap::new_builder()
            .height(VPack::pack(&case.height))
            .header(header.pack())
            .build();

        let service = DummyService::bootstrap(case.height, header).unwrap();
        (service, bootstrap)
    };

    let outputs_data = {
        let spv_info = packed::SpvInfo::new_builder().build();
        let mut outputs_data = vec![spv_info.as_bytes()];
        let mut client = service.tip_client();
        for id in 0..case.clients_count {
            client.id = id;
            let packed_client: packed::SpvClient = client.pack();
            outputs_data.push(packed_client.as_bytes());
        }
        outputs_data
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

    let input = {
        let capacity = SPV_CELL_CAP * (u64::from(case.clients_count) + 1);
        let output = CellOutput::new_builder()
            .capacity(capacity.pack())
            .lock(lock_script.clone())
            .build();
        let out_point = context.create_cell(output, Bytes::new());
        CellInput::new_builder().previous_output(out_point).build()
    };

    let type_script = {
        let cells_count = usize::from(case.clients_count) + 1;
        let type_id_array = utilities::calculate_type_id(input.clone(), cells_count);
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

    let outputs = {
        let spv_cell = CellOutput::new_builder()
            .capacity(SPV_CELL_CAP.pack())
            .lock(lock_script.clone())
            .type_(Some(type_script.clone()).pack())
            .build();
        vec![spv_cell.clone(); usize::from(case.clients_count) + 1]
    };

    let witness = {
        let type_args = BytesOpt::new_builder()
            .set(Some(Pack::pack(bootstrap.as_slice())))
            .build();
        let witness_args = WitnessArgs::new_builder().output_type(type_args).build();
        witness_args.as_bytes()
    };

    let tx = TransactionBuilder::default()
        .input(input)
        .outputs(outputs)
        .outputs_data(outputs_data.pack())
        .witness(Pack::pack(&witness))
        .build();
    let tx = context.complete_tx(tx);

    (context, tx)
}

// Every mutation starts from a VM-verified transaction, so a failed setup cannot
// masquerade as rejection of the malformed field under test.
fn valid_create() -> (Context, TransactionView) {
    let (context, tx) = build_create(NormalCase {
        headers_path: "case-0822528_0830592",
        height: 822528,
        clients_count: 3,
    });
    context
        .verify_tx(&tx, MAX_CYCLES)
        .expect("valid create fixture");
    (context, tx)
}

fn reject_create(context: &Context, tx: &TransactionView, code: i8) {
    use ckb_testtool::ckb_script::{ScriptError, TransactionScriptError};
    let expected = ScriptError::validation_failure(
        &tx.outputs().get(0).unwrap().type_().to_opt().unwrap(),
        code,
    )
    .output_type_script(0);
    let actual = context
        .verify_tx(tx, MAX_CYCLES)
        .expect_err("invalid create");
    assert_eq!(
        actual.root_cause().downcast_ref::<TransactionScriptError>(),
        Some(&expected)
    );
}

fn replace_data(tx: &TransactionView, index: usize, data: Bytes) -> TransactionView {
    let mut outputs_data: Vec<Bytes> = tx
        .outputs_data()
        .into_iter()
        .map(|d| d.raw_data())
        .collect();
    outputs_data[index] = data;
    tx.as_advanced_builder()
        .set_outputs_data(outputs_data.into_iter().map(|d| Pack::pack(&d)).collect())
        .build()
}

#[test]
fn rejects_nonzero_initial_tip() {
    let (context, tx) = valid_create();
    let info = packed::SpvInfo::new_builder()
        .tip_client_id(1.into())
        .build();
    reject_create(&context, &replace_data(&tx, 0, info.as_bytes()), 0x25);
}

#[test]
fn rejects_malformed_info_and_client_data() {
    let (context, tx) = valid_create();
    reject_create(&context, &replace_data(&tx, 0, Bytes::new()), 0x24);
    reject_create(&context, &replace_data(&tx, 1, Bytes::new()), 0x27);
}

#[test]
fn rejects_client_state_not_derived_from_bootstrap() {
    let (context, tx) = valid_create();
    let client =
        packed::SpvClient::from_slice(&tx.outputs_data().get(1).unwrap().raw_data()).unwrap();
    let wrong = client.clone().as_builder().id(2.into()).build();
    reject_create(&context, &replace_data(&tx, 1, wrong.as_bytes()), 0x28);
    let wrong_tip = client
        .as_builder()
        .tip_block_hash(core::Hash::from_bytes_ref(&[0xff; 32]).pack())
        .build();
    reject_create(&context, &replace_data(&tx, 1, wrong_tip.as_bytes()), 0x28);
}

#[test]
fn rejects_missing_and_malformed_bootstrap() {
    let (context, tx) = valid_create();
    let absent = tx
        .as_advanced_builder()
        .set_witnesses(vec![Pack::pack(&WitnessArgs::default().as_bytes())])
        .build();
    reject_create(&context, &absent, 0x26);
    let bad = WitnessArgs::new_builder()
        .output_type(Some(Bytes::from_static(&[1, 2])).pack())
        .build();
    let malformed = tx
        .as_advanced_builder()
        .set_witnesses(vec![Pack::pack(&bad.as_bytes())])
        .build();
    reject_create(&context, &malformed, 0x04);
}

#[test]
fn rejects_incorrect_type_id_and_client_count() {
    let (context, tx) = valid_create();
    let script = tx.outputs().get(0).unwrap().type_().to_opt().unwrap();
    let args = packed::SpvTypeArgs::from_slice(&script.args().raw_data()).unwrap();
    let wrong_id = args
        .clone()
        .as_builder()
        .type_id(core::Hash::from_bytes_ref(&[0xff; 32]).pack())
        .build();
    let wrong_count = args.as_builder().clients_count(4.into()).build();
    for (args, code) in [(wrong_id, 0x23), (wrong_count, 0x22)] {
        let changed = script
            .clone()
            .as_builder()
            .args(args.as_slice().pack())
            .build();
        let outputs = tx
            .outputs()
            .into_iter()
            .map(|o| o.as_builder().type_(Some(changed.clone()).pack()).build())
            .collect();
        let mutated = tx.as_advanced_builder().set_outputs(outputs).build();
        reject_create(&context, &mutated, code);
    }
}

#[test]
fn rejects_missing_client_and_nonconsecutive_cells() {
    let (context, tx) = valid_create();
    // Keep all capacity and data; remove only one cell from the type group.
    let mut outputs: Vec<CellOutput> = tx.outputs().into_iter().collect();
    outputs[3] = outputs[3]
        .clone()
        .as_builder()
        .type_(None::<Script>.pack())
        .build();
    let missing = tx.as_advanced_builder().set_outputs(outputs).build();
    reject_create(&context, &missing, 0x20);

    let mut outputs: Vec<CellOutput> = tx.outputs().into_iter().collect();
    let unrelated = outputs[0]
        .clone()
        .as_builder()
        .type_(None::<Script>.pack())
        .build();
    outputs.insert(1, unrelated);
    let mut data: Vec<_> = tx.outputs_data().into_iter().collect();
    data.insert(1, Pack::pack(&Bytes::new()));
    let gap = tx
        .as_advanced_builder()
        .set_outputs(outputs)
        .set_outputs_data(data)
        .build();
    reject_create(&context, &gap, 0x21);
}

#[test]
fn rejects_invalid_bootstrap_height_and_pow_in_vm() {
    use ckb_bitcoin_spv_verifier::error::BootstrapError;
    let (context, tx) = valid_create();
    let witness = WitnessArgs::from_slice(&tx.witnesses().get(0).unwrap().raw_data()).unwrap();
    let bootstrap =
        packed::SpvBootstrap::from_slice(&witness.output_type().to_opt().unwrap().raw_data())
            .unwrap();
    let wrong_height = bootstrap
        .clone()
        .as_builder()
        .height(VPack::pack(&822529u32))
        .build();
    let mut header: core::Header =
        bitcoin::consensus::deserialize(&bootstrap.header().raw_data()).unwrap();
    header.bits = bitcoin::pow::CompactTarget::from_consensus(0);
    let invalid_pow = bootstrap.as_builder().header(header.pack()).build();
    for (bootstrap, error) in [
        (wrong_height, BootstrapError::Height),
        (invalid_pow, BootstrapError::Pow),
    ] {
        let witness = witness
            .clone()
            .as_builder()
            .output_type(Some(Bytes::copy_from_slice(bootstrap.as_slice())).pack())
            .build();
        let mutated = tx
            .as_advanced_builder()
            .set_witnesses(vec![Pack::pack(&witness.as_bytes())])
            .build();
        reject_create(&context, &mutated, 0x60 + error as i8);
    }
}
