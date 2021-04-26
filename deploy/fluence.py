from __future__             import with_statement
from collections            import namedtuple
from fabric.api             import *
from fabric.contrib.files   import append
from utils                  import *
from collections            import namedtuple
from time                   import sleep


# DTOs
Node = namedtuple("Node", "peer_id tcp ws")
Service = namedtuple("Service", "port multiaddr")

FLUENCE_NODE_PORT = "7777"
FLUENCE_CLIENT_PORT = "9999"
PEER_ID_MARKER = "server peer id"


@task
@runs_once
def deploy_fluence():
    # 'running', 'output'
    with hide():
        load_config()
        target = target_environment()
        env.hosts = target["bootstrap"]

        puts("Fluence: deploying bootstrap")
        special_nodes = deploy_bootstrap()
        bootstrap = special_nodes[0]
        puts("Fluence: bootstrap will be {}".format(bootstrap.tcp.multiaddr))
        env.bootstrap = bootstrap.tcp.multiaddr

        env.hosts = env.config["nodes"]
        puts("Fluence: deploying others")
        result = execute(do_deploy_fluence)
        nodes = fill_addresses(result.items())
        puts("Fluence: deployed.\nAddresses:\n%s" % "\n".join(
            "{} {} {}".format(n.tcp.multiaddr, n.ws.multiaddr, n.peer_id) for n in nodes))
        puts("Bootstrap:\n{} {} {}".format(bootstrap.tcp.multiaddr, bootstrap.ws.multiaddr, bootstrap.peer_id))
        puts("Special ones:\n%s" % "\n".join(
            "{} {} {}".format(n.tcp.multiaddr, n.ws.multiaddr, n.peer_id) for n in special_nodes[1:]))


def deploy_bootstrap():
    results = execute(do_deploy_bootstrap)
    nodes = fill_addresses(results.items())
    return nodes


@task
def do_deploy_bootstrap():
    return do_deploy_fluence(yml="fluence_bootstrap.yml")


@task
@parallel
# returns {ip: Node}
def do_deploy_fluence(yml="fluence.yml"):
    with hide():
        put(yml, './')
        kwargs = {'HOST': env.host_string, 'TAG': docker_tag()}
        if 'bootstrap' in env:
            kwargs['BOOTSTRAP'] = env.bootstrap

        with shell_env(**kwargs):
            # compose('config', yml)
            compose("pull", yml)
            compose('rm -fs', yml)
            compose('up --no-start', yml)  # was: 'create'
            copy_configs_and_keys(yml)
            compose("restart", yml)
            sleep(1)
            addrs = get_fluence_addresses(yml)
            return addrs


def get_host_idx(containers):
    return env.hosts.index(env.host_string) * containers

def copy_key(yml, container, idx):
    keypair = get_keypair(yml, idx)
    fname = '{}_{}.key'.format(yml, idx)
    append(fname, keypair)
    run('docker cp %s %s:/node.key' % (fname, container))

def copy_configs_and_keys(yml):
    put("Config.toml", "./")
    containers = compose('ps -q', yml).splitlines()
    host_idx = get_host_idx(len(containers))
    for idx, id in enumerate(containers):
        run('docker cp ./Config.toml %s:/Config.toml' % id)
        copy_key(yml, id, host_idx + idx)

# returns [Node]
def get_fluence_addresses(yml="fluence.yml"):
    containers = compose('ps -q', yml).splitlines()
    nodes = []
    for id in containers:
        (tcp_port, ws_port) = get_ports(id)
        peer_id = get_fluence_peer_ids(id)
        node = Node(peer_id=peer_id, tcp=Service(tcp_port, None), ws=Service(ws_port, None))
        nodes.append(node)
    return nodes

# Assuming Fluence's tcp port starts with 7
# and websocket port starts with 9
def is_fluence_port(host_port):
    is_tcp = '0.0.0.0:7' in host_port
    is_ws = '0.0.0.0:9' in host_port
    return is_tcp or is_ws

# returns (tcp port, ws port)
def get_ports(container):
    from itertools import chain
    lines = run('docker port %s' % container).splitlines()
    ports = chain.from_iterable(l.split('/tcp -> ') for l in lines)
    # filter by host port and remove 0.0.0.0 part
    ports = list(port.replace('0.0.0.0:', '') for port in ports if is_fluence_port(port))
    (a, b) = ports
    # tcp port starts with 7
    if a.startswith('7'):
        return (a, b)
    else:
        return (b, a)


def get_fluence_peer_ids(container, yml="fluence.yml"):
    logs = run('docker logs %s' % container).splitlines()
    return parse_peer_ids(logs)


# returns (node_peer_id, peer_peer_id)
def parse_peer_ids(logs):
    def after_eq(line):
        return line.split("=")[-1].strip()

    peer_id = None
    for line in logs:
        if PEER_ID_MARKER in line:
            peer_id = after_eq(line)
    return peer_id


def compose(cmd, yml="fluence.yml"):
    return run('docker-compose -f %s %s' % (yml, cmd))


def service(yml):
    return yml.replace(".yml", "")


# takes: dict {ip: Node}
# returns: [Node]
def fill_addresses(nodes_dict):
    result = []
    for ip, nodes in nodes_dict:
        for node in nodes:
            # node service multiaddr
            node = node._replace(tcp=fill_multiaddr(ip, node.tcp))
            # peer service multiaddr
            node = node._replace(ws=fill_multiaddr(ip, node.ws, suffix="/ws"))
            result.append(node)
    return result


def fill_multiaddr(ip, service, suffix=""):
    return service._replace(multiaddr="/ip4/{}/tcp/{}{}".format(ip, service.port, suffix))
