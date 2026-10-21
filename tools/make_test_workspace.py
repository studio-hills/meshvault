"""Builds a small Meshvault workspace for trying the viewer and for the
ignored export tests (MESHVAULT_WS): one animated model, TestRig, whose head
is the skinned sample mesh 13674780763. With --schema 2 (the default) it also
has coloured primitive parts: ball, cylinder, wedge, corner wedge and a
SpecialMesh sphere.

Usage: python tools/make_test_workspace.py OUT [--schema 1|2] [--force]
OUT must not exist yet; --force replaces an existing workspace there."""
import argparse
import json
import math
import os
import shutil
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SAMPLES = os.path.join(REPO, 'crates', 'rbx_mesh', 'meshes')
# keep in step with MANIFEST_VERSION in crates/meshvault/src/workspace/mod.rs
MANIFEST_VERSION = 3
IDENTITY = [1, 0, 0, 0, 1, 0, 0, 0, 1]

parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
parser.add_argument('out')
parser.add_argument('--schema', type=int, choices=(1, 2), default=2)
parser.add_argument('--force', action='store_true', help='replace an existing workspace at OUT')
args = parser.parse_args()
out = args.out

if os.path.exists(out):
    if not args.force:
        sys.exit(f'{out} exists; pass --force to replace it')
    # only ever delete something that is a workspace
    if not os.path.isfile(os.path.join(out, 'workspace.json')):
        sys.exit(f'{out} is not a Meshvault workspace; not replacing it')
    shutil.rmtree(out)
os.makedirs(os.path.join(out, 'Models'))
os.makedirs(os.path.join(out, 'Rigs', 'Workspace'))
shutil.copy(os.path.join(SAMPLES, '13674780763'), os.path.join(out, 'Models', 'Head.mesh'))


def cf(x=0.0, y=0.0, z=0.0, rx=0.0):
    c, s = math.cos(rx), math.sin(rx)
    # rotation about X, row-major
    return [x, y, z, 1, 0, 0, 0, c, -s, 0, s, c]


def pose(name, weight, transform, children=()):
    return {'Name': name, 'Transform': transform, 'Weight': weight,
            'EasingStyle': 'Linear', 'EasingDirection': 'In', 'Children': list(children)}


def frame(t, nod, jaw):
    return {'Time': t, 'Markers': [{'Name': 'Peak', 'Value': ''}] if t == 0.5 else [],
            'Poses': [pose('HumanoidRootPart', 0, cf(), [
                pose('UpperTorso', 0, cf(), [
                    pose('Head', 1, cf(rx=nod), [pose('jaw', 1, cf(rx=jaw))])])])]}


def primitive(name, cls, shape, x, size, color, special=None):
    p = {'Path': 'Workspace/TestRig/' + name, 'Name': name, 'MeshRef': 0, 'ClassName': cls, 'Shape': shape,
         'CFrame': [x, 0.5, -2] + IDENTITY, 'Size': size, 'Color': color, 'BrickColor': name,
         'Material': 'Plastic', 'Transparency': 0, 'Reflectance': 0}
    if special:
        p['SpecialMesh'] = special
    return p


model = {
    'Name': 'TestRig', 'Path': 'Workspace/TestRig',
    'Parts': [
        {'Path': 'Workspace/TestRig/HumanoidRootPart', 'Name': 'HumanoidRootPart', 'MeshRef': 0,
         'CFrame': cf(0, 0, 0), 'Size': [2, 2, 1], 'Transparency': 1},
        {'Path': 'Workspace/TestRig/UpperTorso', 'Name': 'UpperTorso', 'MeshRef': 0,
         'CFrame': cf(0, 0, 0), 'Size': [2, 1.6, 1], 'Transparency': 0},
        {'Path': 'Workspace/TestRig/Head', 'Name': 'Head', 'MeshRef': 1,
         'CFrame': cf(0, 1.4, 0), 'Size': [1.2, 1.2, 1.2], 'Transparency': 0},
    ],
    'Joints': [
        {'Name': 'Root', 'Path': 'Workspace/TestRig/UpperTorso/Root',
         'Part0': 'Workspace/TestRig/HumanoidRootPart', 'Part1': 'Workspace/TestRig/UpperTorso',
         'C0': cf(), 'C1': cf()},
        {'Name': 'Neck', 'Path': 'Workspace/TestRig/Head/Neck',
         'Part0': 'Workspace/TestRig/UpperTorso', 'Part1': 'Workspace/TestRig/Head',
         'C0': cf(0, 0.8, 0), 'C1': cf(0, -0.6, 0)},
    ],
    'Bones': [],
    'Animations': [
        {'Name': 'Nod and talk', 'AnimationId': 'rbxassetid://1', 'Status': 'KeyframeSequence',
         'Duration': 1.0, 'Loop': True, 'Priority': 'Action', 'Curves': [],
         'Keyframes': [frame(0.0, 0.0, 0.0), frame(0.5, -0.45, 0.35), frame(1.0, 0.0, 0.0)]},
        {'Name': 'Private dance', 'AnimationId': 'rbxassetid://2', 'Status': 'Unavailable',
         'Duration': 0, 'Loop': False, 'Priority': '', 'Keyframes': [], 'Curves': []},
    ],
}

if args.schema == 2:
    model['Scale'] = 1
    for p in model['Parts']:
        p.update({'ClassName': 'MeshPart' if p['MeshRef'] else 'Part', 'Shape': '' if p['MeshRef'] else 'Block',
                  'Color': [0.35, 0.55, 0.9] if p['Name'] == 'UpperTorso' else [0.9, 0.8, 0.6],
                  'BrickColor': 'Test', 'Material': 'Plastic', 'Reflectance': 0})
    model['Parts'] += [
        primitive('Ball', 'Part', 'Ball', -3, [1.5, 1.5, 1.5], [0.9, 0.3, 0.3]),
        primitive('Cylinder', 'Part', 'Cylinder', -1, [2, 1, 1], [0.3, 0.8, 0.4]),
        primitive('Wedge', 'WedgePart', 'Wedge', 1, [1, 1, 1.5], [0.95, 0.75, 0.2]),
        primitive('CornerWedge', 'CornerWedgePart', 'CornerWedge', 2.5, [1, 1, 1], [0.6, 0.4, 0.9]),
        primitive('Egg', 'Part', 'Block', 4, [1, 1, 1], [0.95, 0.95, 0.95],
                  {'MeshType': 'Sphere', 'Scale': [1, 1.5, 1], 'Offset': [0, 0.25, 0]}),
    ]
    for j in model['Joints']:
        j['Transform'] = [0, 0, 0] + IDENTITY

mesh = {'Id': 1, 'Name': 'Head', 'MeshId': 'rbxassetid://13674780763', 'TextureId': '', 'Folder': 'Workspace/TestRig',
        'ColorMap': '', 'NormalMap': '', 'RoughnessMap': '', 'MetalnessMap': ''}
rig_file = 'Rigs/Workspace/TestRig.rig.json'
with open(os.path.join(out, rig_file), 'w') as f:
    json.dump({'SchemaVersion': args.schema, 'Meshes': [mesh], 'Models': [model]}, f)
manifest = {
    'version': MANIFEST_VERSION, 'name': 'Rig test',
    'items': [{'name': 'Head', 'folder': ['Workspace', 'TestRig'], 'mesh_id': 13674780763, 'stem': 'Models/Head',
               'mesh': {'state': 'done', 'path': 'Models/Head.mesh'}, 'maps': {}}],
    'models': [{'name': 'TestRig', 'path': 'Workspace/TestRig', 'folder': ['Workspace'], 'file': rig_file,
                'meshes': {'1': 'Models/Head'}, 'parts': len(model['Parts']), 'clips': 2, 'playable_clips': 1}],
}
with open(os.path.join(out, 'workspace.json'), 'w') as f:
    json.dump(manifest, f, indent=2)
print(f'ok, schema {args.schema}: {out}')
