# Copyright 2018 Fluence Labs Limited
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

#!/bin/bash

set -e

# The script uses for deploying Parity, Swarm, and Fluence containers.
# If `PROD_DEPLOY` is set in env, the script will also expect the following env variables: `NAME`, `PORTS`, `OWNER_ADDRESS`, `PRIVATE_KEY`
# Without `PROD_DEPLOY` exported flag the script will use default arguments
# If first arg is `multiple`, script will start 4 fluence node along with Swarm & Parity nodes

# generates json with all arguments required for registration a node
function generate_json()
{
    # printing the command in the last line to parse it from a control script
    DATA="{\"node_ip\": \"$EXTERNAL_HOST_IP\",
           \"tendermint_key\": \"$TENDERMINT_KEY\",
           \"tendermint_node_id\": \"$TENDERMINT_NODE_ID\",
           \"contract_address\": \"$CONTRACT_ADDRESS\",
           \"account\": \"$OWNER_ADDRESS\",
           \"start_port\": $START_PORT,
           \"last_port\": $LAST_PORT}"
    JSON=$(echo $DATA | paste -sd "" - | awk 'NF')
    echo $JSON
}

function generate_command()
{
    echo "./fluence register \
            --node_ip            $EXTERNAL_HOST_IP \
            --tendermint_key     $TENDERMINT_KEY \
            --tendermint_node_id $TENDERMINT_NODE_ID \
            --contract_address   $CONTRACT_ADDRESS \
            --account            $OWNER_ADDRESS \
            --secret_key         $PRIVATE_KEY \
            --start_port         $START_PORT \
            --last_port          $LAST_PORT \
            --eth_url            http://$EXTERNAL_HOST_IP:8545 \
            --wait_syncing \
            --base64_tendermint_key"
}

function parse_tendermint_params()
{
    local __TENDERMINT_KEY=$1
    local __TENDERMINT_NODE_ID=$2

    # get tendermint key from node logs
    # todo get this from `status` API by CLI
    while [ -z "$TENDERMINT_KEY" -o -z "$TENDERMINT_NODE_ID" ]; do
        local DOCKER_STATUS=$(docker ps -a --filter "name=fluence-node-$COUNTER" --format '{{.Status}}' | grep -o Exited)
        if [ -z "$DOCKER_STATUS" ]
        then
            echo -e "\e[91m'fluence-node-'$COUNTER container cannot be run\e[0m"
            exit 127
        fi
        # TODO: parse for 'Node ID' instead of 'PubKey'
        TENDERMINT_KEY=$(docker logs fluence-node-$COUNTER 2>&1 | awk 'match($0, /PubKey: /) { print substr($0, RSTART + RLENGTH) }')
        TENDERMINT_NODE_ID=$(docker logs fluence-node-$COUNTER 2>&1 | awk 'match($0, /Node ID: /) { print substr($0, RSTART + RLENGTH) }')
        sleep 3
    done

    eval $__TENDERMINT_KEY="'$TENDERMINT_KEY'"
    eval $__TENDERMINT_NODE_ID="'$TENDERMINT_NODE_ID'"
}

function deploy_contract_locally()
{
    if [ ! -d "node_modules" ]; then
        npm install
    fi
    RESULT=$(npm run deploy)
    # get last word from script output
    local CONTRACT_ADDRESS=`echo ${RESULT} | awk '{print $NF}'`
    sleep 1

    echo $CONTRACT_ADDRESS
}

# Updates all needed containers
function container_update()
{
    docker pull parity/parity:v2.3.0
    docker pull ethdevops/swarm:edge
    docker pull fluencelabs/node:latest
    docker pull fluencelabs/worker:latest
}

function get_docker_ip_address()
{
    # getting docker ip address
    case "$(uname -s)" in
       Darwin)
         export DOCKER_IP=host.docker.internal
         ;;

       Linux)
         export DOCKER_IP=$(ifconfig docker0 | grep 'inet ' | awk '{print $2}' | grep -Po "[0-9\.]+")
         ;;
    esac
}

function export_arguments()
{
    if [ -z "$PROD_DEPLOY" ]; then
        echo "Deploying locally with default arguments."
        export NAME='fluence-node-1'
        # open 10 ports, so it's possible to create 10 workers
        export PORTS='25000:25010'
        # eth address in `dev` mode Parity with eth
        export OWNER_ADDRESS=0x00a329c0648769a73afac7f9381e08fb43dbea72
        export PRIVATE_KEY=4d5db4107d237df6a3d58ee5f70ae63d73d7658d4026f2eefd2f204c81682cb7
        export PARITY_ARGS='--config dev-insecure --jsonrpc-apis=all --jsonrpc-hosts=all --jsonrpc-cors="*" --unsafe-expose'
    else
        echo "Deploying for $CHAIN chain."
        export PARITY_ARGS='--light --chain '$CHAIN' --jsonrpc-apis=all --jsonrpc-hosts=all --jsonrpc-cors="*"'
    fi
}

function get_external_ips()
{
    # use exported external ip address or get it from OS
    # todo rewrite this
    if [ -z "$PROD_DEPLOY" ]; then
        EXTERNAL_HOST_IP="127.0.0.1"
        case "$(uname -s)" in
           Darwin)
             export HOST_IP=host.docker.internal
             ;;

           Linux)
             export HOST_IP=$(ip route get 8.8.8.8 | grep -Po "(?<=src )[0-9\.]+")
             ;;
        esac
    else
        EXTERNAL_HOST_IP=$HOST_IP
    fi
}

function start_parity_swarm()
{
    # running parity and swarm containers
    docker-compose -f parity.yml up -d
    docker-compose -f swarm.yml up -d

    # waiting that API of parity start working
    # todo get rid of all `sleep`
    sleep 30

    echo 'Parity and Swarm containers are started.'
}

# ================================================================
# ====================== SCRIPT STARTS HERE ======================
# ================================================================

container_update

# `PROD_DEPLOY` variable is assigned in `fabfile.py`, so if run `compose.sh` directly,
#  the network will be started in development mode locally

# exports initial arguments to global scope for `docker-compose` files
export_arguments

get_docker_ip_address

get_external_ips

start_parity_swarm

# deploy contract if there is new dev ethereum node
if [ -z "$PROD_DEPLOY" ]; then
    export CONTRACT_ADDRESS=$(deploy_contract_locally)
fi

START_PORT=${PORTS%:*}
LAST_PORT=${PORTS#*:}
export STATUS_PORT=$((LAST_PORT+400))

# check all variables exists
echo "CONTRACT_ADDRESS="$CONTRACT_ADDRESS
echo "NAME="$NAME
echo "PORTS="$PORTS
echo "HOST_IP="$HOST_IP
echo "EXTERNAL_HOST_IP="$EXTERNAL_HOST_IP
echo "OWNER_ADDRESS="$OWNER_ADDRESS
echo "CONTRACT_ADDRESS="$CONTRACT_ADDRESS
echo "PRIVATE_KEY="$PRIVATE_KEY

# port for status API
echo "STATUS_PORT="$STATUS_PORT

COUNTER=1

# starting node container
# if there was `multiple` flag on the running script, will be created 4 nodes, otherwise one node
if [ "$1" = "multiple" ]; then
    docker-compose -f multiple-node.yml up -d --force-recreate
    NUMBER_OF_NODES=4
else
    docker-compose -f node.yml up -d --force-recreate
    NUMBER_OF_NODES=1
fi

echo 'Node container is started.'

while [ $COUNTER -le $NUMBER_OF_NODES ]; do
    parse_tendermint_params TENDERMINT_KEY TENDERMINT_NODE_ID

    echo "CURRENT NODE = "$COUNTER
    echo "TENDERMINT_KEY="$TENDERMINT_KEY
    echo "TENDERMINT_NODE_ID="$TENDERMINT_NODE_ID

    # use hardcoded ports for multiple nodes
    if [ "$1" = "multiple" ]; then
        START_PORT="25"$COUNTER"00"
        LAST_PORT="25"$COUNTER"10"
    fi

    echo "START_PORT="$START_PORT
    echo "LAST_PORT="$LAST_PORT

    echo "Registering node in smart contract:"

    # registers node in Fluence contract, for local usage
    if [ -z "$PROD_DEPLOY" ]; then
        set -x
        REGISTER_COMMAND=$(generate_command)
        eval $REGISTER_COMMAND
        set +x
    fi

    # generates JSON with all arguments for node registration
    JSON=$(generate_json)
    echo $JSON

    COUNTER=$[$COUNTER+1]
    TENDERMINT_KEY=""
done
