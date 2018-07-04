/*
 * Copyright (C) 2017  Fluence Labs Limited
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU Affero General Public License as
 * published by the Free Software Foundation, either version 3 of the
 * License, or (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU Affero General Public License for more details.
 *
 * You should have received a copy of the GNU Affero General Public License
 * along with this program.  If not, see <http://www.gnu.org/licenses/>.
 */

package fluence.contract.grpc.client

import cats.effect.IO
import fluence.codec.Codec
import fluence.contract.ops.ContractValidate
import fluence.contract.protobuf.BasicContract
import fluence.contract.protocol.ContractAllocatorRpc
import fluence.crypto.signature.SignAlgo.CheckerFn
import fluence.stream.Connection
import monix.execution.Scheduler

/**
 * Contract allocator client.
 *
 */
class ContractAllocatorClient[C: ContractValidate](connection: Connection)(
  implicit
  codec: Codec[IO, C, BasicContract],
  checkerFn: CheckerFn,
  ec: Scheduler
) extends ContractAllocatorRpc[C] {
  import ContractValidate.ContractValidatorOps
  import fluence.transport.ProtobufCodec._

  private val service = "fluence.contract.protobuf.grpc.ContractAllocator"

  /**
   * Offer a contract. Node should check and preallocate required resources, save offer, and sign it.
   *
   * @param contract A blank contract
   * @return Signed contract, or F is an error
   */
  override def offer(contract: C): IO[C] =
    for {
      // we should validate contract before send outside for 'offering'
      _ ← contract.validateME[IO]
      offer ← codec.encode(contract)
      request ← generatedMessageCodec.runF[IO](offer)
      responseBytes ← connection
        .handleUnary(service, "offer", request)
      resp ← protobufDynamicCodec(BasicContract).runF[IO](responseBytes)
      respContract ← codec.decode(resp)
      // contract from the outside required validation
      _ ← respContract.validateME[IO]
    } yield respContract

  /**
   * Allocate dataset: store the contract, create storage structures, form cluster.
   *
   * @param contract A sealed contract with all nodes and client signatures
   * @return Allocated contract
   */
  override def allocate(contract: C): IO[C] =
    for {
      // we should validate contract before send outside for 'allocating'
      _ ← contract.validateME[IO]
      offer ← codec.encode(contract)
      request ← generatedMessageCodec.runF[IO](offer)
      responseBytes ← connection
        .handleUnary(service, "allocate", request)
      resp ← protobufDynamicCodec(BasicContract).runF[IO](responseBytes)
      respContract ← codec.decode(resp)
      // contract from the outside required validation
      _ ← respContract.validateME[IO]
    } yield respContract

}

object ContractAllocatorClient {
  def apply[C : ContractValidate](streamHandler: Connection)(
    implicit
    codec: Codec[IO, C, BasicContract],
    checkerFn: CheckerFn,
    ec: Scheduler
  ): ContractAllocatorRpc[C] = new ContractAllocatorClient(streamHandler)
}
