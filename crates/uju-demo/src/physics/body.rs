use parry3d::glamx::Vec3;
use parry3d::mass_properties::MassProperties;
use uju::ecs::Component;

#[derive(Component)]
pub enum RigidBody {
    Dynamic,
    Kinematic,
    Static,
}

// todo: extract or merge with `uju` crates velocity structs
#[derive(Component)]
pub struct Velocity {
    pub linear: Vec3,
    pub angular: Vec3,
}

#[derive(Component)]
pub struct Mass(pub MassProperties);
