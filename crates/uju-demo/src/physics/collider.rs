use uju::ecs::Component;

#[derive(Component)]
pub struct Collider {
    pub shape: parry3d::shape::SharedShape,
}
