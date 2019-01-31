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

package fluence.node.workers.tendermint.rpc

import cats.data.EitherT
import cats.effect.{Concurrent, Resource, Sync}
import cats.syntax.apply._
import cats.syntax.flatMap._
import cats.syntax.either._
import cats.syntax.functor._
import com.softwaremill.sttp._
import com.softwaremill.sttp.circe.asJson
import fluence.node.workers.WorkerParams
import cats.syntax.applicativeError._
import fluence.node.workers.tendermint.status.StatusResponse
import io.circe.generic.semiauto._
import io.circe.{Encoder, Json}

import scala.language.higherKinds

/**
 * Provides a single concurrent endpoint to run RPC requests on Worker
 *
 * @param sinkRpc Tendermint's RPC requests endpoint
 * @param status Tendermint's status
 * @tparam F Concurrent effect
 */
case class TendermintRpc[F[_]] private (
  sinkRpc: fs2.Sink[F, TendermintRpc.Request],
  status: EitherT[F, Throwable, StatusResponse.WorkerTendermintInfo]
) {

  val broadcastTxCommit: fs2.Sink[F, String] =
    (s: fs2.Stream[F, String]) ⇒ s.map(TendermintRpc.broadcastTxCommit(_)) to sinkRpc

  /**
   * Make a single RPC call in a fire-and-forget manner.
   * Response is to be dropped, so you should take care of `id` in the request if you need to get it
   *
   * @param req The Tendermint request
   * @param F Concurrent effect
   */
  def callRpc(req: TendermintRpc.Request)(implicit F: Concurrent[F]): F[Unit] =
    fs2.Stream(req).to(sinkRpc).compile.drain

}

object TendermintRpc {
  private val requestEncoder: Encoder[Request] = deriveEncoder[Request]

  /**
   * Wrapper for Tendermint's RPC request
   *
   * @param method Method name
   * @param jsonrpc Version of the JSON RPC protocol
   * @param params Sequence of arguments for the method
   * @param id Nonce to track the results of the request with some other method
   */
  case class Request(method: String, jsonrpc: String = "2.0", params: Seq[Json], id: String = "") {
    def toJsonString: String = requestEncoder(this).noSpaces
  }

  /**
   * Builds a broadcast_tx_commit RPC request
   *
   * @param tx Transaction body
   * @param id Tracking ID, you may omit it
   * NOTE from Tendermint docs: it is not possible to send transactions to Tendermint during `Commit` - if your app tries to send a `/broadcast_tx` to Tendermint during Commit, it will deadlock.
   * TODO: ensure the above deadlock doesn't happen
   */
  def broadcastTxCommit(tx: String, id: String = ""): Request =
    Request(method = "broadcast_tx_commit", params = Json.fromString(tx) :: Nil, id = id)

  private def toSomePipe[F[_], T]: fs2.Pipe[F, T, Option[T]] =
    _.map(Some(_))

  private def status[F[_]: Sync](
    params: WorkerParams
  )(implicit sttpBackend: SttpBackend[F, Nothing]): EitherT[F, Throwable, StatusResponse.WorkerTendermintInfo] =
    EitherT {
      val url = uri"http://${params.currentWorker.ip.getHostAddress}:${params.currentWorker.rpcPort}/status"

      sttp
        .get(url)
        .response(asJson[StatusResponse])
        .send()
        .attempt
        // converting Either[Throwable, Response[Either[DeserializationError[circe.Error], WorkerResponse]]]
        // to Either[Throwable, WorkerResponse]
        .map(
          _.flatMap(
            _.body
              .leftMap(new Exception(_))
              .flatMap(_.leftMap(_.error))
          )
        )
    }.map(_.result)

  /**
   * Runs a WorkerRpc with F effect, acquiring some resources for it
   *
   * @param params Worker params to get Worker URI from
   * @param sttpBackend Sttp Backend to be used to make RPC calls
   * @tparam F Concurrent effect
   * @return Worker RPC instance. Note that it should be stopped at some point, and can't be used after it's stopped
   */
  def apply[F[_]: Concurrent](
    params: WorkerParams
  )(implicit sttpBackend: SttpBackend[F, Nothing]): Resource[F, TendermintRpc[F]] =
    Resource
      .make(
        for {
          queue ← fs2.concurrent.Queue.noneTerminated[F, TendermintRpc.Request]
          fiber ← Concurrent[F].start(
            queue.dequeue
              .evalMap(
                req ⇒
                  sttpBackend
                    .send(
                      sttp
                        .post(uri"http://${params.currentWorker.ip.getHostAddress}:${params.currentWorker.rpcPort}/")
                        .body(req.toJsonString)
                    )
                    .map(_.isSuccess)
              )
              .drain
              .compile
              .drain
          )

          enqueue = queue.enqueue
          stop = fs2.Stream(None).to(enqueue).compile.drain *> fiber.join
          callRpc = toSomePipe[F, TendermintRpc.Request].andThen(_ to enqueue)

        } yield
          new TendermintRpc[F](
            callRpc,
            status[F](params)
          ) → stop
      ) {
        case (_, stop) ⇒ stop
      }
      .map(_._1)
}
