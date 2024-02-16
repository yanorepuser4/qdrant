import multiprocessing
import pathlib
from time import sleep

from .fixtures import upsert_random_points, create_collection
from .utils import *

N_PEERS = 3
N_SHARDS = 3
N_REPLICA = 1
COLLECTION_NAME = "test_collection"


def update_points_in_loop(peer_url, collection_name, offset=0, throttle=False, duration=None):
    start = time.time()
    limit = 3

    while True:
        upsert_random_points(peer_url, limit, collection_name, offset=offset)
        offset += limit

        print(f"Inserted at offset {offset}")

        if throttle:
            sleep(0.1)
        if duration is not None and (time.time() - start) > duration:
            break


def run_update_points_in_background(peer_url, collection_name, init_offset=0, throttle=False, duration=None):
    p = multiprocessing.Process(target=update_points_in_loop, args=(peer_url, collection_name, init_offset, throttle, duration))
    p.start()
    return p


# Transfer shards from one node to another
#
# Simply does the most basic transfer: no concurrent updates during the
# transfer.
#
# Test that data on the both sides is consistent
def test_shard_snapshot_transfer(tmp_path: pathlib.Path):
    assert_project_root()

    # seed port to reuse the same port for the restarted nodes
    peer_api_uris, peer_dirs, bootstrap_uri = start_cluster(tmp_path, N_PEERS, 20000)

    create_collection(peer_api_uris[0], shard_number=N_SHARDS, replication_factor=N_REPLICA)
    wait_collection_exists_and_active_on_all_peers(
        collection_name=COLLECTION_NAME,
        peer_api_uris=peer_api_uris
    )

    # Insert some initial number of points
    upsert_random_points(peer_api_uris[0], 100)

    transfer_collection_cluster_info = get_collection_cluster_info(peer_api_uris[0], COLLECTION_NAME)
    receiver_collection_cluster_info = get_collection_cluster_info(peer_api_uris[2], COLLECTION_NAME)

    from_peer_id = transfer_collection_cluster_info['peer_id']
    to_peer_id = receiver_collection_cluster_info['peer_id']

    shard_id = transfer_collection_cluster_info['local_shards'][0]['shard_id']

    # Transfer shard from one node to another

    # Move shard `shard_id` to peer `target_peer_id`
    r = requests.post(
        f"{peer_api_uris[0]}/collections/{COLLECTION_NAME}/cluster", json={
            "replicate_shard": {
                "shard_id": shard_id,
                "from_peer_id": from_peer_id,
                "to_peer_id": to_peer_id,
                "method": "snapshot"
            }
        })
    assert_http_ok(r)

    # Wait for end of shard transfer
    wait_for_collection_shard_transfers_count(peer_api_uris[0], COLLECTION_NAME, 0)

    receiver_collection_cluster_info = get_collection_cluster_info(peer_api_uris[2], COLLECTION_NAME)
    number_local_shards = len(receiver_collection_cluster_info['local_shards'])
    assert number_local_shards == 2

    # Point counts must be consistent across nodes
    counts = []
    for uri in peer_api_uris:
        r = requests.post(
            f"{uri}/collections/{COLLECTION_NAME}/points/count", json={
                "exact": True
            }
        )
        assert_http_ok(r)
        counts.append(r.json()["result"]['count'])
    assert counts[0] == counts[1] == counts[2]


def load_all_points(peer_api_uri: str):

    next_page_offset = 0

    all_points_ids = set()

    while next_page_offset is not None:

        r = requests.post(
            f"{peer_api_uri}/collections/{COLLECTION_NAME}/points/scroll", json={
                "limit": 1000,
                "offset": next_page_offset
            }
        )
        assert_http_ok(r)

        points = r.json()["result"]["points"]
        next_page_offset = r.json()["result"]["next_page_offset"]

        for point in points:
            all_points_ids.add(point["id"])
    
    return all_points_ids


# Transfer shards from one node to another while applying throttled updates in parallel
#
# Updates are throttled to prevent sending updates faster than the queue proxy
# can handle. The transfer must therefore finish in 30 seconds without issues.
#
# Test that data on the both sides is consistent
def test_shard_snapshot_transfer_throttled_updates(tmp_path: pathlib.Path):
    assert_project_root()

    # seed port to reuse the same port for the restarted nodes
    peer_api_uris, peer_dirs, bootstrap_uri = start_cluster(tmp_path, N_PEERS, 20000)

    create_collection(peer_api_uris[0], shard_number=N_SHARDS, replication_factor=N_REPLICA)
    wait_collection_exists_and_active_on_all_peers(
        collection_name=COLLECTION_NAME,
        peer_api_uris=peer_api_uris
    )

    # Insert some initial number of points
    upsert_random_points(peer_api_uris[0], 10000)

    # Start pushing points to the cluster
    upload_process_1 = run_update_points_in_background(peer_api_uris[0], COLLECTION_NAME, init_offset=100, throttle=True)
    upload_process_2 = run_update_points_in_background(peer_api_uris[1], COLLECTION_NAME, init_offset=10000, throttle=True)
    upload_process_3 = run_update_points_in_background(peer_api_uris[2], COLLECTION_NAME, init_offset=20000, throttle=True)

    transfer_collection_cluster_info = get_collection_cluster_info(peer_api_uris[0], COLLECTION_NAME)
    receiver_collection_cluster_info = get_collection_cluster_info(peer_api_uris[2], COLLECTION_NAME)

    from_peer_id = transfer_collection_cluster_info['peer_id']
    to_peer_id = receiver_collection_cluster_info['peer_id']

    shard_id = transfer_collection_cluster_info['local_shards'][0]['shard_id']

    # Transfer shard from one node to another

    # Move shard `shard_id` to peer `target_peer_id`
    r = requests.post(
        f"{peer_api_uris[0]}/collections/{COLLECTION_NAME}/cluster", json={
            "replicate_shard": {
                "shard_id": shard_id,
                "from_peer_id": from_peer_id,
                "to_peer_id": to_peer_id,
                "method": "snapshot"
            }
        })
    assert_http_ok(r)

    # Wait for end of shard transfer
    wait_for_collection_shard_transfers_count(peer_api_uris[0], COLLECTION_NAME, 0)

    upload_process_1.kill()
    upload_process_2.kill()
    upload_process_3.kill()
    sleep(1)

    receiver_collection_cluster_info = get_collection_cluster_info(peer_api_uris[2], COLLECTION_NAME)
    number_local_shards = len(receiver_collection_cluster_info['local_shards'])
    assert number_local_shards == 2

    # Point counts must be consistent across nodes
    counts = []
    for uri in peer_api_uris:
        r = requests.post(
            f"{uri}/collections/{COLLECTION_NAME}/points/count", json={
                "exact": True
            }
        )
        assert_http_ok(r)
        counts.append(r.json()["result"]['count'])
    assert counts[0] == counts[1] == counts[2]


# Transfer shards from one node to another while applying updates in parallel
#
# A fast burst of updates is sent in the first 5 seconds, the queue proxy will
# not be able to keep up with this. After that, updates are throttled. The
# transfer must still finish in 30 seconds without issues.
#
# Test that data on the both sides is consistent
def test_shard_snapshot_transfer_fast_burst(tmp_path: pathlib.Path):
    assert_project_root()

    # seed port to reuse the same port for the restarted nodes
    peer_api_uris, peer_dirs, bootstrap_uri = start_cluster(tmp_path, N_PEERS, 20000)

    create_collection(peer_api_uris[0], shard_number=N_SHARDS, replication_factor=N_REPLICA)
    wait_collection_exists_and_active_on_all_peers(
        collection_name=COLLECTION_NAME,
        peer_api_uris=peer_api_uris
    )

    # Insert some initial number of points
    upsert_random_points(peer_api_uris[0], 10000)

    # Start pushing points to the cluster
    upload_process_1 = run_update_points_in_background(peer_api_uris[0], COLLECTION_NAME, init_offset=100000, duration=5)
    upload_process_2 = run_update_points_in_background(peer_api_uris[1], COLLECTION_NAME, init_offset=200000, duration=5)
    upload_process_3 = run_update_points_in_background(peer_api_uris[2], COLLECTION_NAME, init_offset=300000, throttle=True)

    transfer_collection_cluster_info = get_collection_cluster_info(peer_api_uris[0], COLLECTION_NAME)
    receiver_collection_cluster_info = get_collection_cluster_info(peer_api_uris[2], COLLECTION_NAME)

    from_peer_id = transfer_collection_cluster_info['peer_id']
    to_peer_id = receiver_collection_cluster_info['peer_id']

    shard_id = transfer_collection_cluster_info['local_shards'][0]['shard_id']

    # Transfer shard from one node to another

    # Move shard `shard_id` to peer `target_peer_id`
    r = requests.post(
        f"{peer_api_uris[0]}/collections/{COLLECTION_NAME}/cluster", json={
            "replicate_shard": {
                "shard_id": shard_id,
                "from_peer_id": from_peer_id,
                "to_peer_id": to_peer_id,
                "method": "snapshot"
            }
        })
    assert_http_ok(r)

    # Wait for end of shard transfer
    wait_for_collection_shard_transfers_count(peer_api_uris[0], COLLECTION_NAME, 0)

    upload_process_1.kill()
    upload_process_2.kill()
    upload_process_3.kill()
    # sleep(1)

    receiver_collection_cluster_info = get_collection_cluster_info(peer_api_uris[2], COLLECTION_NAME)
    number_local_shards = len(receiver_collection_cluster_info['local_shards'])
    assert number_local_shards == 2

    # Point counts must be consistent across nodes
    counts = []
    for uri in peer_api_uris:
        r = requests.post(
            f"{uri}/collections/{COLLECTION_NAME}/points/count", json={
                "exact": True
            }
        )
        assert_http_ok(r)
        counts.append(r.json()["result"]['count'])


    if not (counts[0] == counts[1] == counts[2]):
        print(f"counts: {counts}")

        uri_0_points = load_all_points(peer_api_uris[0])
        uri_1_points = load_all_points(peer_api_uris[1])
        uri_2_points = load_all_points(peer_api_uris[2])

        print(f"uri_0_points: {len(uri_0_points)}")
        print(f"uri_1_points: {len(uri_1_points)}")
        print(f"uri_2_points: {len(uri_2_points)}")

        print(f"uri_0_points - uri_1_points: {uri_0_points - uri_1_points}")
        print(f"uri_0_points - uri_2_points: {uri_0_points - uri_2_points}")
        print(f"uri_1_points - uri_0_points: {uri_1_points - uri_0_points}")

    assert counts[0] == counts[1] == counts[2], f"counts: {counts}"
