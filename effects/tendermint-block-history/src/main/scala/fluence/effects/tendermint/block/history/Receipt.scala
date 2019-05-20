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

package fluence.effects.tendermint.block.history

import io.circe.generic.semiauto.{deriveDecoder, deriveEncoder}
import io.circe.{Decoder, Encoder}
import scodec.bits.ByteVector

import scala.language.higherKinds

case class Receipt(hash: ByteVector) {

  // TODO: serialize to JSON, and get bytes
  def bytes(): ByteVector = {
    import io.circe.syntax._
    ByteVector((this: Receipt).asJson.noSpaces.getBytes())
  }
}

object Receipt {
  private implicit val decbc: Decoder[ByteVector] =
    Decoder.decodeString.flatMap(
      ByteVector.fromHex(_).fold(Decoder.failedWithMessage[ByteVector]("Not a hex"))(Decoder.const)
    )

  private implicit val encbc: Encoder[ByteVector] = Encoder.encodeString.contramap(_.toHex)

  implicit val dec: Decoder[Receipt] = deriveDecoder[Receipt]
  implicit val enc: Encoder[Receipt] = deriveEncoder[Receipt]
}
