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

package fluence.swarm.responses
import io.circe.{Decoder, Encoder}
import io.circe.generic.semiauto.{deriveDecoder, deriveEncoder}

/**
 * One file in the manifest with additional information.
 *
 * @param hash address of file in swarm
 * @param contentType type of stored data (tar, multipart, etc)
 * @param mod_time time of modification
 */
case class Entrie(hash: String, contentType: String, mod_time: String)

object Entrie {
  implicit val fooDecoder: Decoder[Entrie] = deriveDecoder
  implicit val fooEncoder: Encoder[Entrie] = deriveEncoder
}

/**
 * Representation of raw manifest with entries.
 *
 * @param entries list of files under manifest
 */
case class RawResponse(entries: List[Entrie])

case object RawResponse {
  implicit val fooDecoder: Decoder[RawResponse] = deriveDecoder
  implicit val fooEncoder: Encoder[RawResponse] = deriveEncoder
}
