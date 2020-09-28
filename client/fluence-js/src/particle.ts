/*
 * Copyright 2020 Fluence Labs Limited
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

import { v4 as uuidv4 } from 'uuid';
import PeerId from "peer-id";
import {encode} from "bs58";

export interface Particle {
    id: string,
    init_peer_id: string,
    timestamp: bigint,
    ttl: number,
    script: string,
    // sign upper fields
    signature: string,
    data: object,
    action: "Particle"
}

/**
 * Copies a particle and stringify it.
 */
export function stringifyParticle(call: Particle): string {
    let obj: any = {...call};

    return JSON.stringify(obj)
}


export function parseParticle(str: string): Particle {
    let json = JSON.parse(str);

    return {
        id: json.id,
        init_peer_id: json.init_peer_id,
        timestamp: json.timestamp,
        ttl: json.ttl,
        script: json.script,
        signature: json.signature,
        data: json.data,
        action: "Particle"
    }
}

export function canonicalBytes(particle: Particle) {
    let peerIdBuf = Buffer.from(particle.init_peer_id, 'utf8');
    let idBuf = Buffer.from(particle.id, 'utf8');

    let tsArr = new ArrayBuffer(8);
    new DataView(tsArr).setBigUint64(0, particle.timestamp);
    let tsBuf = new Buffer(tsArr);

    let ttlArr = new ArrayBuffer(4);
    new DataView(ttlArr).setUint32(0, particle.ttl);
    let ttlBuf = new Buffer(ttlArr);

    let scriptBuf = Buffer.from(particle.script, 'utf8');

    return Buffer.concat([peerIdBuf, idBuf, tsBuf, ttlBuf, scriptBuf]);
}

/**
 * Sign a particle with a private key from peerId.
 */
export async function signParticle(peerId: PeerId,
                                   particle: Particle): Promise<string> {
    let bufToSign = canonicalBytes(particle);

    let signature = await peerId.privKey.sign(bufToSign)
    return encode(signature)
}

export function genUUID() {
    return uuidv4();
}
