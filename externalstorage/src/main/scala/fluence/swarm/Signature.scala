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

package fluence.swarm
import cats.{Id, Monad}
import cats.data.EitherT
import fluence.crypto.Crypto.Hasher
import fluence.swarm.ECDSASigner.Signer
import io.circe.Encoder
import scodec.bits.ByteVector
import scodec.codecs._
import shapeless.HList

import scala.language.{higherKinds, implicitConversions}

/**
 * Signature helps to identify and ascertain ownership of this resource.
 * Update chunks must carry a rootAddr reference and metaHash in order to be verified.
 * This way, a node that receives an update can check the signature, recover the public address
 * and check the ownership by computing H(ownerAddr, metaHash) and comparing it to the rootAddr
 * the resource is claiming to update without having to lookup the metadata chunk.
 * It is a sign of `digest`.
 * `digest = H(period|version|rootAddr|metaHash|multihash|data)`
 * Where H() is SHA3.
 * @see https://swarm-guide.readthedocs.io/en/latest/usage.html#mutable-resource-updates
 */
case class Signature private (signature: ByteVector)

object Signature extends slogging.LazyLogging {

  import fluence.swarm.helpers.AttemptOps._
  import SwarmConstants._

  implicit val signatureEncoder: Encoder[Signature] = ByteVectorCodec.encodeByteVector.contramap(_.signature)

  private val codec = short16L :: short16L :: int32L :: int32L :: bytes :: bytes :: bool(8) :: bytes

  /**
   *
   * @param period is encoded as little-endian long
   * @param version is encoded as little-endian long
   * @param rootAddr is encoded as a 32 byte array
   * @param metaHash is encoded as a 32 byte array
   * @param multiHash is encoded as the least significant bit of a flags byte
   * @param data is the plain data byte array
   * @return Digest signature.
   */
  def apply[F[_]: Monad](
    period: Int,
    version: Int,
    rootAddr: RootAddr,
    metaHash: MetaHash,
    multiHash: Boolean,
    data: ByteVector,
    signer: Signer[ByteVector, ByteVector]
  )(implicit hasher: Hasher[ByteVector, ByteVector]): EitherT[F, SwarmError, Signature] = {
    for {
      bytes <- codec
        .encode(
          HList(
            updateHeaderLength,
            data.size.toShort,
            period,
            version,
            rootAddr.addr,
            metaHash.hash,
            multiHash,
            data
          )
        )
        .map(_.toByteVector)
        .toEitherT(er => SwarmError(s"Error on encoding signature. ${er.messageWithContext}"))

      _ = logger.debug(
        s"Generate signature of period: $period, version: $version, rootAddr: ${rootAddr.addr}," +
          s"metaHash: ${metaHash.hash}, multiHash: $multiHash, data: $data"
      )

      digestHash <- hasher(bytes).leftMap(er => SwarmError("Error on hashing signature.", Some(er)))

      _ = logger.debug(s"Digest hash on generating signature: $digestHash")

      sign <- signer(digestHash).leftMap(er => SwarmError("Error on sign.", Some(er)))

      _ = logger.debug(s"Generated sign: $sign")
    } yield Signature(sign)
  }
}
