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

package fluence.statemachine

import cats.effect.{IO, Resource}
import fluence.effects.tendermint.block.history.Receipt
import fluence.statemachine.control.{BlockReceipt, ControlSignals, DropPeer, ReceiptType}
import scodec.bits.ByteVector

trait TestControlSignals extends ControlSignals[IO] {
  override val dropPeers: Resource[IO, Set[DropPeer]] =
    Resource.liftF(IO(throw new NotImplementedError("val dropPeers")))
  override val stop: IO[Unit] = IO(throw new NotImplementedError("val stop"))
  override val receipt: IO[BlockReceipt] = IO(throw new NotImplementedError("val receipt"))
  override def putVmHash(hash: ByteVector): IO[Unit] = IO(throw new NotImplementedError("df putVmHash"))
  override def setVmHash(hash: ByteVector): IO[Unit] = IO(throw new NotImplementedError("def setVmHash"))
  override def dropPeer(drop: DropPeer): IO[Unit] = IO(throw new NotImplementedError("def dropPeer"))
  override def stopWorker(): IO[Unit] = IO(throw new NotImplementedError("def stopWorker"))
  override def putReceipt(receipt: BlockReceipt): IO[Unit] = IO(throw new NotImplementedError("def putReceipt"))
  override val vmHash: IO[ByteVector] = IO(throw new NotImplementedError("val vmHash"))
}
