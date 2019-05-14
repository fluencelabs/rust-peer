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

package fluence.kad.protocol

import cats.effect.IO

import scala.concurrent.duration.Duration

/**
 * Provides access to contact-specific services
 *
 * @param rpc           Function to perform request to remote contact
 * @param pingExpiresIn Duration when no ping requests are made by the bucket, to avoid overflows
 * @param check Test node correctness, e.g. signatures are correct, ip is public, etc.
 * @tparam C Contact class
 */
class ContactAccess[C](
  val pingExpiresIn: Duration,
  val check: Node[C] ⇒ IO[Boolean],
  val rpc: C ⇒ KademliaRpc[C]
)

object ContactAccess {
  def apply[C](implicit ca: ContactAccess[C]): ContactAccess[C] = ca
}
