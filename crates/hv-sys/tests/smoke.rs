// One VM per process: this is the only in-process VM test in the crate.
#[test]
fn create_and_destroy_a_vm() {
    let vm = hv_sys::Vm::create().expect("hv_vm_create should succeed when entitled");
    let vcpu = hv_sys::Vcpu::create(&vm).expect("hv_vcpu_create");
    drop(vcpu);
    drop(vm); // Drop calls hv_vm_destroy
}
