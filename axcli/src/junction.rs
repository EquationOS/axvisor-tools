mod elf;

use crate::JunctionArgs;

fn split_args(junction_args: &[String]) -> (Vec<String>, Vec<String>) {
    // Split the junction_args into two parts:
    // 1. The part before the first "--" (junction arguments)
    // 2. The part after the first "--" (application arguments)
    // If "--" is not present, the second part will be empty.
    if let Some(pos) = junction_args.iter().position(|s| s == "--") {
        let junc = junction_args[..pos].to_vec();
        let app = junction_args[pos + 1..].to_vec();
        (junc, app)
    } else {
        (junction_args.to_vec(), vec![])
    }
}

pub fn create_junction(args: JunctionArgs) {
    info!("Create junction with args: {:?}", args.junction_args);
    let (junction_args, app_args) = split_args(&args.junction_args);
    println!("junction_run args: {:?}", junction_args);
    println!("target app args: {:?}", app_args);

    elf::load_junction(&junction_args, &app_args);

    // Here you would implement the logic to create a junction
    // based on the provided arguments.
    // This is a placeholder for the actual implementation.

    // For example, you might parse the arguments and call a hypervisor function:
    // let result = hvc_create_junction(args.junction_args);

    // if result < 0 {
    //     panic!("Failed to create junction: {}", result);
    // }

    info!("Junction created successfully");
}
