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

package fluence.swarm.helpers
import cats.ApplicativeError
import cats.syntax.applicativeError._
import cats.data.EitherT
import com.softwaremill.sttp.Response

import scala.language.higherKinds

object ResponseOps {
  implicit class RichResponse[F[_], T](resp: F[Response[T]])(implicit F: ApplicativeError[F, Throwable]) {
    val toEitherT: EitherT[F, String, T] = resp.attemptT.leftMap(_.getMessage).subflatMap(_.body)
    def toEitherT[E](errFunc: String => E): EitherT[F, E, T] = toEitherT.leftMap(errFunc)
  }
}
