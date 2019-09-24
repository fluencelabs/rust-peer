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

package fluence.node.workers

import fluence.bp.api.BlockProducer
import fluence.bp.tendermint.Tendermint
import fluence.statemachine.api.command.{PeersControl, ReceiptBus}
import fluence.statemachine.api.StateMachine
import fluence.worker.api.WorkerStatus
import fluence.worker.responder.WorkerResponder

import scala.concurrent.duration.FiniteDuration
import scala.language.higherKinds

// Algebra for WorkerServices
trait WorkerServices[F[_]] {
  // Used BlockProducer
  def producer: BlockProducer[F]

  // The underlying StateMachine
  def machine: StateMachine[F]

  // Used by Ethereum to remove peers
  def peersControl: PeersControl[F]

  // Retrieves worker's health
  def status(timeout: FiniteDuration): F[WorkerStatus]

  // Service to subscribe for a response on request
  def responder: WorkerResponder[F]
}
