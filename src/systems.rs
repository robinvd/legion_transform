//! Scene graph system and types

use crate::{
    components::{Parent, Transform},
    ecs::prelude::*,
    math::Matrix4,
};
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
struct TreeNode {
    pub entity: Entity,
    pub children: Vec<TreeNode>,
}

impl TreeNode {
    pub fn new(entity: Entity) -> Self {
        TreeNode {
            entity,
            children: Vec::new(),
        }
    }
}

pub struct TransformSystem {}

impl TransformSystem {
    pub fn new() -> Self {
        TransformSystem {}
    }

    pub fn run_now(&self, world: &World) {
        let mut forest: HashMap<Entity, TreeNode> = HashMap::new();
        let mut visited: HashSet<Entity> = HashSet::new();

        let mut query =
            <(Read<Transform>, Tagged<Parent>)>::query().filter(changed::<Tagged<Parent>>());
        for (entity, _) in query.iter_entities(world) {
            TransformSystem::explore_tree_dfs(entity, &mut forest, &mut visited, world);
        }

        let mut query = <(Read<Transform>)>::query().filter(changed::<Transform>());
        for (entity, _) in query.iter_entities(world) {
            TransformSystem::explore_tree_dfs(entity, &mut forest, &mut visited, world);
        }

        // At this point the forest of transforms that need to be re-computed is built, we can
        // par_iter over it recursively and rebuild the `global_matrix` for each.
        let trees: Vec<_> = forest.values().collect();
        trees
            // .into_par_iter()
            .into_iter()
            .for_each(|tree| TransformSystem::rebuild_recursive(tree, None, world));
    }

    #[inline]
    fn explore_tree_dfs(
        entity: Entity,
        forest: &mut HashMap<Entity, TreeNode>,
        visited: &mut HashSet<Entity>,
        world: &World,
    ) {
        // If the node was visited already, then continue on.
        if visited.contains(&entity) {
            return;
        }

        // Explore it DFS, which will rotate any nodes it comes across that are already roots in
        // the forest into the tree.
        let mut node = TreeNode::new(entity);
        TransformSystem::explore_dfs(&mut node, forest, visited, world);

        // Add it both the forest root and mark it visited.
        forest.insert(entity, node);
        visited.insert(entity);
    }

    #[inline]
    fn explore_dfs(
        parent_node: &mut TreeNode,
        forest: &mut HashMap<Entity, TreeNode>,
        visited: &mut HashSet<Entity>,
        world: &World,
    ) {
        // Iterate children with Transforms.
        let parent = Parent(parent_node.entity);
        let mut children_query = <(Read<Transform>)>::query().filter(tag_value(&parent));
        for (child_entity, _) in children_query.iter_entities(world) {
            // Regardless of it the child is visited, if it's in the root of forest we need to
            // rotate the entire tree to a child of the parent node.
            if let Some(node) = forest.remove(&child_entity) {
                // Add the entire tree under the root and return.
                parent_node.children.push(node);
                return;
            }

            // This node was visited already but isn't the root of a tree then stop searching.
            if visited.contains(&child_entity) {
                return;
            }

            // Visit the child recursively.
            visited.insert(child_entity);
            let mut child_node = TreeNode::new(child_entity);
            TransformSystem::explore_dfs(&mut child_node, forest, visited, world);
            parent_node.children.push(child_node);
        }
    }

    #[inline]
    fn rebuild_recursive(node: &TreeNode, parent_matrix: Option<Matrix4<f32>>, world: &World) {
        let global_matrix = {
            if let Some(parent_matrix) = parent_matrix {
                let mut transform = world.get_component_mut::<Transform>(node.entity).unwrap();
                transform.global_matrix = parent_matrix * transform.matrix();
                transform.global_matrix
            } else {
                let mut transform = world.get_component_mut::<Transform>(node.entity).unwrap();
                transform.global_matrix = transform.matrix();
                transform.global_matrix
            }
        };

        // Re-compute any children in parallel.
        // node.children.par_iter().for_each(|child| {
        node.children.iter().for_each(|child| {
            TransformSystem::rebuild_recursive(child, Some(global_matrix), world)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::TransformSystem;
    use crate::{
        components::{Parent, Transform},
        ecs::prelude::*,
        math::{Matrix4, Quaternion, Translation3, Unit, UnitQuaternion, Vector3},
    };
    use approx::*;
    use std::f32::consts::PI;

    #[test]
    fn transform_matrix() {
        let mut transform = Transform::default();
        transform.set_translation_xyz(5.0, 2.0, -0.5);
        transform.set_rotation(Unit::new_normalize(Quaternion::new(1.0, 0.0, 0.0, 0.0)));
        transform.set_scale(Vector3::new(2.0, 2.0, 2.0));

        let combined = Matrix4::new_translation(transform.translation())
            * transform.rotation().to_rotation_matrix().to_homogeneous()
            * Matrix4::new_scaling(2.0);

        assert_eq!(transform.matrix(), combined);
    }

    fn transform_world() -> (World, TransformSystem) {
        let universe = Universe::new();
        let world = universe.create_world();
        (world, TransformSystem::new())
    }

    fn together(global_matrix: Matrix4<f32>, local_matrix: Matrix4<f32>) -> Matrix4<f32> {
        global_matrix * local_matrix
    }

    // Basic default Transform's local matrix -> global matrix  (Should just be identity)
    #[test]
    fn zeroed() {
        let (mut world, system) = transform_world();

        let transform = Transform::default();

        let e1 = *world.insert((), vec![(transform,)]).first().unwrap();

        system.run_now(&world);

        let transform = world.get_component::<Transform>(e1).unwrap();
        // let a1: [[f32; 4]; 4] = transform.global_matrix().into();
        // let a2: [[f32; 4]; 4] = Transform::default().global_matrix().into();
        assert_eq!(
            transform.global_matrix(),
            Transform::default().global_matrix()
        );
    }

    // Basic sanity check for Transform's local matrix -> global matrix, no parent relationships
    //
    // Should just put the value of the Transform's local matrix into the global matrix field.
    #[test]
    fn basic() {
        let (mut world, system) = transform_world();

        let mut local = Transform::default();
        local.set_translation_xyz(5.0, 5.0, 5.0);
        local.set_rotation(Unit::new_normalize(Quaternion::new(1.0, 0.5, 0.5, 0.0)));

        let e1 = *world.insert((), vec![(local.clone(),)]).first().unwrap();

        system.run_now(&world);

        let transform = world.get_component::<Transform>(e1).unwrap();
        let a1 = transform.global_matrix();
        let a2 = local.matrix();
        assert_eq!(*a1, a2);
    }

    // Test Parent's global matrix * Child's local matrix -> Child's global matrix (Parent is before child)
    #[test]
    fn parent_before() {
        let (mut world, system) = transform_world();

        let mut local1 = Transform::default();
        local1.set_translation_xyz(5.0, 5.0, 5.0);
        local1.set_rotation(Unit::new_normalize(Quaternion::new(1.0, 0.5, 0.5, 0.0)));

        let e1 = *world.insert((), vec![(local1.clone(),)]).first().unwrap();

        let mut local2 = Transform::default();
        local2.set_translation_xyz(5.0, 5.0, 5.0);
        local2.set_rotation(Unit::new_normalize(Quaternion::new(1.0, 0.5, 0.5, 0.0)));

        let e2 = *world
            .insert((Parent(e1),), vec![(local2.clone(),)])
            .first()
            .unwrap();

        let mut local3 = Transform::default();
        local3.set_translation_xyz(5.0, 5.0, 5.0);
        local3.set_rotation(Unit::new_normalize(Quaternion::new(1.0, 0.5, 0.5, 0.0)));

        let e3 = *world
            .insert((Parent(e2),), vec![(local3.clone(),)])
            .first()
            .unwrap();

        system.run_now(&world);

        let e1_transform = world.get_component::<Transform>(e1).unwrap();
        let a1 = e1_transform.global_matrix();
        let a2 = local1.matrix();
        assert_eq!(*a1, a2);

        let e2_transform = world.get_component::<Transform>(e2).unwrap();
        let a3 = e2_transform.global_matrix();
        let a4 = together(*a1, local2.matrix());
        assert_eq!(*a3, a4);

        let e3_transform = world.get_component::<Transform>(e3).unwrap();
        let a3 = e3_transform.global_matrix();
        let _a4 = together(*a3, local3.matrix());
    }

    /// Tests that re-parenting transforms correctly causes descendants to be re-computed.
    #[test]
    fn reparenting() {
        let system = TransformSystem::new();
        let mut world = Universe::new().create_world();

        // Create a translation and a rotation transform.
        let forward_one_transform = Transform::from(Vector3::new(1.0, 0.0, 0.0));
        let mut rotate_right_90 = Transform::default();
        rotate_right_90.set_rotation_euler(0.0, PI, 0.0);

        // Make 2 forward transforms, and 1 rotation
        let e = world.insert(
            (),
            vec![
                (forward_one_transform.clone(),),
                (forward_one_transform.clone(),),
                (rotate_right_90.clone(),),
            ],
        );
        let [fwd1, fwd2, rot] = [e[0], e[1], e[2]];

        // Run the System without any parenting
        system.run_now(&world);

        // Assert it didn't change any of the transforms (none of them had parents).
        assert_relative_eq!(
            world
                .get_component::<Transform>(fwd1)
                .unwrap()
                .global_matrix,
            Translation3::new(1.0, 0.0, 0.0).into(),
            max_relative = 0.000_001,
        );
        assert_relative_eq!(
            world
                .get_component::<Transform>(fwd2)
                .unwrap()
                .global_matrix,
            Translation3::new(1.0, 0.0, 0.0).into(),
            max_relative = 0.000_001,
        );
        assert_relative_eq!(
            world.get_component::<Transform>(rot).unwrap().global_matrix,
            UnitQuaternion::from_euler_angles(0.0, PI, 0.0).into(),
            max_relative = 0.000_001,
        );

        // Create 2 tree:
        // - Rot
        // - fwd1 -> fwd2
        world.add_tag(fwd2, Parent(fwd1));
        system.run_now(&world);

        // Global matrix of fwd2 should be double the distance, the rest should be the same.
        assert_relative_eq!(
            world
                .get_component::<Transform>(fwd1)
                .unwrap()
                .global_matrix,
            Translation3::new(1.0, 0.0, 0.0).into(),
            max_relative = 0.000_001,
        );
        assert_relative_eq!(
            world
                .get_component::<Transform>(fwd2)
                .unwrap()
                .global_matrix,
            Translation3::new(2.0, 0.0, 0.0).into(),
            max_relative = 0.000_001,
        );
        assert_relative_eq!(
            world.get_component::<Transform>(rot).unwrap().global_matrix,
            UnitQuaternion::from_euler_angles(0.0, PI, 0.0).into(),
            max_relative = 0.000_001,
        );

        // Re-parent to the tree
        // - fwd1 -> Rot -> fwd2
        world.add_tag(fwd2, Parent(rot));
        world.add_tag(rot, Parent(fwd1));
        system.run_now(&world);

        assert_relative_eq!(
            world
                .get_component::<Transform>(fwd1)
                .unwrap()
                .global_matrix,
            Translation3::new(1.0, 0.0, 0.0).into(),
            max_relative = 0.000_001,
        );
        assert_relative_eq!(
            world
                .get_component::<Transform>(fwd2)
                .unwrap()
                .global_matrix,
            (Translation3::new(1.0, 0.0, 0.0)
                * UnitQuaternion::from_euler_angles(0.0, PI, 0.0)
                * Translation3::new(1.0, 0.0, 0.0))
            .into(),
            max_relative = 0.000_001,
        );
        assert_relative_eq!(
            world.get_component::<Transform>(rot).unwrap().global_matrix,
            (Translation3::new(1.0, 0.0, 0.0) * UnitQuaternion::from_euler_angles(0.0, PI, 0.0))
                .into(),
            max_relative = 0.000_001,
        );

        // Un-parent and re-parent into the two trees:
        // - fwd1
        // - rot -> fw2
        world.remove_tag::<Parent>(fwd1);
        world.remove_tag::<Parent>(rot);
        world.add_tag(fwd2, Parent(rot));
        system.run_now(&world);

        assert_relative_eq!(
            world
                .get_component::<Transform>(fwd1)
                .unwrap()
                .global_matrix,
            Translation3::new(1.0, 0.0, 0.0).into(),
            max_relative = 0.000_001,
        );
        assert_relative_eq!(
            world
                .get_component::<Transform>(fwd2)
                .unwrap()
                .global_matrix,
            (UnitQuaternion::from_euler_angles(0.0, PI, 0.0) * Translation3::new(1.0, 0.0, 0.0))
                .into(),
            max_relative = 0.000_001,
        );
        assert_relative_eq!(
            world.get_component::<Transform>(rot).unwrap().global_matrix,
            UnitQuaternion::from_euler_angles(0.0, PI, 0.0).into(),
            max_relative = 0.000_001,
        );
    }
}
