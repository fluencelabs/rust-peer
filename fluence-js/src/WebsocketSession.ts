/*
 * Copyright 2018 Fluence Labs Limited
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

import {genRequestId, genSessionId, prepareRequest, PrivateKey} from "./utils";
import {Node} from "./contract";
import {toByteArray} from "base64-js";
import {Result} from "./Result";
import {Executor, ExecutorType, PromiseExecutor, SubscribtionExecutor} from "./executor";

let debug = require('debug');

interface WebsocketResponse {
    request_id: string
    type: string
    data?: string
    error?: string
}

export class WebsocketSession {
    private sessionId: string;
    private appId: string;
    private readonly privateKey?: PrivateKey;
    private counter: number;
    private nodes: Node[];
    private nodeCounter: number;
    private socket: WebSocket;

    // result is for 'txWaitRequest' and 'query', void is for all other requests
    private executors = new Map<string, Executor<Result | void>>();

    // promise, that should be completed if websocket is connected
    private connectionHandler: PromiseExecutor<void>;

    /**
     * Create connected websocket.
     *
     */
    static create(appId: string, nodes: Node[], privateKey?: PrivateKey): Promise<WebsocketSession> {
        let ws = new WebsocketSession(appId, nodes, privateKey);
        return ws.connect();
    }

    private constructor(appId: string, nodes: Node[], privateKey?: PrivateKey) {
        if (nodes.length == 0) {
            console.error("There is no nodes to connect");
            throw new Error("There is no nodes to connect");
        }

        this.counter = 0;
        this.nodeCounter = 0;
        this.sessionId = genSessionId();
        this.appId = appId;
        this.nodes = nodes;
        this.privateKey = privateKey;
    }

    private messageHandler(msg: string) {
        let response;
        try {
            let rawResponse = WebsocketSession.parseRawResponse(msg);

            debug("Message received: " + JSON.stringify(rawResponse));

            if (!this.executors.has(rawResponse.request_id)) {
                console.error(`There is no message with requestId '${rawResponse.request_id}'. Message: ${msg}`)
            } else {
                let executor = this.executors.get(rawResponse.request_id) as Executor<Result>;
                if (rawResponse.error) {
                    console.log(`Error received for ${rawResponse.request_id}: ${JSON.stringify(rawResponse.error)}`)
                    executor.handleError(rawResponse.error as string);
                } else if (rawResponse.type === "tx_wait_response") {
                    if (rawResponse.data) {
                        let parsed = JSON.parse(rawResponse.data).result.response;
                        let result = new Result(toByteArray(parsed.value));

                        executor.handleResult(result)
                    }
                    if (executor.type === "promise") {
                        this.executors.delete(rawResponse.request_id);
                    }
                } else {
                    let executor = this.executors.get(rawResponse.request_id) as Executor<void>;
                    executor.handleResult()
                }
            }


        } catch (e) {
            console.error("Cannot parse websocket event: " + e)
        }
    }

    /**
     * Trying to subscribe to all existed subscriptions again.
     */
    private resubscribe() {
        this.executors.forEach((executor: Executor<any>, key: string) => {
            if (executor.type === ExecutorType.Subscription) {
                let subExecutor = executor as SubscribtionExecutor;
                this.subscribe(subExecutor.subscription, subExecutor.resultHandler, subExecutor.errorHandler)
                    .catch((e) => console.error(`Cannot resubscribe on ${subExecutor.subscription}`))
            }
        });
    }

    /**
     * Creates a new websocket connection. Waits after websocket will become connected.
     */
    private connect(): Promise<WebsocketSession> {
        let node = this.nodes[this.nodeCounter % this.nodes.length];
        this.nodeCounter++;
        this.connectionHandler = new PromiseExecutor<void>();

        debug("Connecting to " + JSON.stringify(node));

        let socket = new WebSocket(`ws://${node.ip_addr}:${node.api_port}/apps/${this.appId}/ws`);

        this.socket = socket;

        socket.onopen = () => {
            debug("Websocket is opened");
            this.connectionHandler.handleResult()
        };

        socket.onerror = (e) => {
            console.error("Websocket receive an error: " + e + ". Reconnecting.");
            this.reconnectSession(e)
        };

        socket.onclose = (e) => {
            console.error("Websocket is closed. Reconnecting.");
            this.reconnectSession(e)
        };

        socket.onmessage = (msg) => {
            this.messageHandler(msg.data)

        };

        return this.connectionHandler.promise().then(() => this);
    }

    /**
     * Increments current internal counter
     */
    private getCounterAndIncrement() {
        return this.counter++;
    }

    /**
     * Delete a subscription.
     *
     */
    async unsubscribe(subscriptionId: string): Promise<void> {

        debug("Unsibscribe " + subscriptionId);

        await this.connectionHandler.promise();

        let requestId = genRequestId();

        let request = {
            request_id: requestId,
            subscription_id: subscriptionId,
            type: "unsubscribe_request"
        };

        await this.sendAndWaitResponse(requestId, JSON.stringify(request));

        this.executors.delete(subscriptionId);
    }

    private async subscribeCall(transaction: string, requestId: string, subscriptionId: string): Promise<Result> {
        let request = {
            tx: transaction,
            request_id: requestId,
            subscription_id: subscriptionId,
            type: "subscribe_request"
        };

        return this.sendAndWaitResponse(requestId, JSON.stringify(request));
    }

    /**
     * Creates a subscription, that will return responses on every change in a state machine.
     * @param transaction will be run on state machine on every change
     * @param resultHandler to handle changes
     * @param errorHandler to handle errors
     */
    async subscribe(transaction: string, resultHandler: (result: Result) => void, errorHandler: (error: any) => void): Promise<string> {
        await this.connectionHandler.promise();
        let requestId = genRequestId();
        let subscriptionId = genRequestId();

        let executor: SubscribtionExecutor = new SubscribtionExecutor(transaction, resultHandler, errorHandler);

        let promise = this.subscribeCall(transaction, requestId, subscriptionId);
        await promise;

        this.executors.set(subscriptionId, executor);

        return promise.then(() => subscriptionId);
    }

    /**
     * Send a request without waiting a response.
     */
    async requestAsync(payload: string): Promise<void> {

        await this.connectionHandler.promise();

        let requestId = genRequestId();
        let counter = this.getCounterAndIncrement();

        let tx = prepareRequest(payload, this.sessionId, counter, this.privateKey);

        let request = {
            tx: tx.payload,
            request_id: requestId,
            type: "tx_request"
        };

        return this.sendAndWaitResponse(requestId, JSON.stringify(request)).then((r) => {});
    }

    /**
     * Send a request and waiting for a response.
     */
    request(payload: string): Promise<Result> {
        let requestId = genRequestId();
        let counter = this.getCounterAndIncrement();

        let tx = prepareRequest(payload, this.sessionId, counter, this.privateKey);

        let request = {
            tx: tx.payload,
            request_id: requestId,
            type: "tx_wait_request"
        };

        console.log("send request: " + JSON.stringify(request));

        return this.sendAndWaitResponse(requestId, JSON.stringify(request))
    }

    /**
     * Send a request to websocket and create a promise that will wait for a response.
     */
    private sendAndWaitResponse(requestId: string, message: string): Promise<Result> {
        this.socket.send(message);

        let executor: PromiseExecutor<Result> = new PromiseExecutor();

        this.executors.set(requestId, executor);

        return executor.promise()
    }

    /**
     * Generate new sessionId, terminate old connectionHandler and create a new one.
     * Terminate all executors that are waiting for responses.
     */
    private reconnectSession(reason: any) {
        this.sessionId = genSessionId();
        this.counter = 0;
        this.connectionHandler.handleError(reason);
        this.connect();

        // terminate and delete all executors that are waiting requests
        this.executors.forEach((executor: Executor<Result | void>, key: string) => {
            if (executor.type === ExecutorType.Promise) {
                executor.handleError("Reconnecting. All waiting requests are terminated.");
                this.executors.delete(key)

            }
        });
    }

    private static parseRawResponse(response: string): WebsocketResponse {
        let parsed = JSON.parse(response);
        if (!parsed.request_id) throw new Error("Cannot parse response, no 'request_id' field.");
        if (parsed.type === "tx_wait_response" && !parsed.data && !parsed.error) throw new Error(`Cannot parse response, no 'data' or 'error' field in response with requestId '${parsed.requestId}'`);

        return parsed as WebsocketResponse;
    }
}
