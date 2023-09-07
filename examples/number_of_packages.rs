use hwlocality::{
    objects::{depth::DepthError, types::ObjectType},
    Topology,
};

/// Prints the number of packages.
fn main() -> anyhow::Result<()> {
    let topology = Topology::new()?;

    match topology.depth_for_type(ObjectType::Package) {
        Ok(depth) => println!(
            "*** Found {} package(s)",
            topology.num_objects_at_depth(depth)
        ),
        Err(DepthError::Nonexistent) => println!("*** No package object found"),
        Err(DepthError::Multiple) => println!("*** Package objects exist at multiple depths"),
        Err(DepthError::Unexpected(i)) => println!("*** Unknown error while probing packages: {i}"),
    }

    Ok(())
}
