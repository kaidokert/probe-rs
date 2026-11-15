use probe_rs::{CoreType, InstructionSet};

use super::{GdbRegisterSource, TargetDescription, build_target_description};
use crate::rpc::functions::core_ops::convert::{armv8m_features, core_registers_from_metadata};
use probe_rs::CoreRegisters;
use probe_rs_rpc::core_ops::{WireCoreMetadata, WireInstructionSet};

#[test]
fn test_target_description_microbit() {
    let target_desc = TargetDescription::new(CoreType::Armv6m, InstructionSet::Thumb2);
    let description = target_desc.get_target_xml();

    insta::assert_snapshot!(description);
}

#[test]
fn test_target_with_features() {
    let mut target_desc = TargetDescription::new(CoreType::Armv6m, InstructionSet::Thumb2);
    target_desc.add_gdb_feature("org.probe-rs.feature1");
    target_desc.add_register_from_details("r0", 32, 0.into());
    target_desc.add_register_from_details("x1", 64, 1.into());
    target_desc.add_register_from_details("t2", 64, 2.into());

    target_desc.update_register_name("t2", "at2");
    target_desc.update_register_type("at2", "special_reg");

    target_desc.add_gdb_feature("org.probe-rs.feature2");
    target_desc.add_register_from_details("v4", 128, 4.into());

    let description = target_desc.get_target_xml();

    insta::assert_snapshot!(description);
}

#[test]
fn armv8m_gdb_secure_stack_names_map_to_physical_registers() {
    for fp in [false, true] {
        let native = CoreRegisters::for_armv8m(true, true, fp);
        let metadata = WireCoreMetadata {
            fpu_support: fp,
            floating_point_register_count: fp.then_some(32),
            instruction_set: WireInstructionSet::Thumb2,
            armv8m_features: armv8m_features(CoreType::Armv8m, native),
        };
        let registers = core_registers_from_metadata(CoreType::Armv8m, &metadata);
        let description =
            build_target_description(registers, CoreType::Armv8m, InstructionSet::Thumb2);
        let xml = description.get_target_xml();
        for (name, hardware_id) in [("msp_s", 26), ("msp_ns", 24), ("msplim_ns", 30)] {
            let needle = format!("<reg name='{name}'");
            let offset = xml.find(&needle).expect("register missing from target XML");
            let index = xml[..offset].matches("<reg ").count();
            assert!(matches!(description.get_register(index).unwrap().source(),
                GdbRegisterSource::SingleRegister(id) if id == probe_rs::RegisterId(hardware_id)));
        }
    }
}
