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

package fluence.effects.tendermint.rpc

import cats.data.EitherT
import cats.effect._
import cats.effect.concurrent.{Deferred, Ref}
import cats.effect.syntax.effect._
import cats.syntax.flatMap._
import cats.syntax.applicativeError._
import cats.syntax.functor._
import cats.syntax.apply._
import fluence.effects.{Backoff, EffectError}
import fluence.effects.JavaFutureConversion._
import fluence.effects.tendermint.rpc.helpers.NettyFutureConversion._
import fs2.concurrent.Queue
import org.asynchttpclient.Dsl._
import org.asynchttpclient.netty.ws.NettyWebSocket
import org.asynchttpclient.ws.{WebSocket, WebSocketListener, WebSocketUpgradeHandler}

import scala.language.higherKinds

/**
 * Implementation of Tendermint RPC Subscribe call
 * Details: https://tendermint.com/rpc/#subscribe
 */
trait WebsocketTendermintRpc extends slogging.LazyLogging {
  import WebsocketTendermintRpc.Disconnected

  val host: String
  val port: Int

  private val wsUrl = s"ws://$host:$port/websocket"

  def subscribeNewBlock[F[_]: ConcurrentEffect: Timer]: Resource[F, fs2.Stream[F, String]] = {
    subscribe("NewBlock").map(_.dequeue.unNoneTerminate)
  }

  private def subscribe[F[_]: ConcurrentEffect: Timer](
    event: String
  ): Resource[F, Queue[F, Option[String]]] = {
    def close(ws: (NettyWebSocket, _)) = ws._1.sendCloseFrame().asAsync.void

    Resource.make {
      for {
        queue <- Queue.unbounded[F, Option[String]]
        ref <- Ref.of[F, String]("")
        websocket <- {
          Backoff.default(EitherT(for {
            // promise will be completed with Right on connect and Left on close
            promise <- Deferred[F, Either[Disconnected.type, Unit]]
            websocket <- connect(wsHandler(event, ref, queue, promise))
            connected <- promise.get
          } yield connected.map(_ => websocket)))
        }
        _ <- websocket.sendTextFrame(request(event)).asAsync
      } yield (websocket, queue)
    }(close).map(_._2)
  }

  private def request(event: String) =
    s"""
       |{
       |    "jsonrpc": "2.0",
       |    "id": "dontcare",
       |    "query": "tm.event = '$event'"
       |}
     """.stripMargin

  private def wsHandler[F[_]: ConcurrentEffect](
    event: String,
    ref: Ref[F, String],
    queue: Queue[F, Option[String]],
    connected: Deferred[F, Either[Disconnected.type, Unit]]
  ) =
    new WebSocketUpgradeHandler.Builder()
      .addWebSocketListener(
        new WebSocketListener {
          override def onOpen(websocket: WebSocket): Unit = {
            logger.info(s"Tendermint WRPC: $wsUrl connected")
            connected.complete(Right(())).toIO.unsafeRunSync()
          }

          override def onClose(websocket: WebSocket, code: Int, reason: String): Unit = {
            logger.warn(s"Tendermint WRPC: $wsUrl closed")
            (connected.complete(Left(Disconnected)).attempt.void *> queue.enqueue1(None)).toIO.unsafeRunSync()
          }

          override def onError(t: Throwable): Unit = {
            logger.error(s"Tendermint WRPC: $wsUrl ${t.getMessage}")
          }

          override def onTextFrame(payload: String, finalFragment: Boolean, rsv: Int): Unit = {
            if (!finalFragment) {
              logger.warn(s"Tendermint WRPC: $wsUrl event was split into several websocket frames")
              ref.update(_.concat(payload)).toIO.unsafeRunSync()
            } else {
              // TODO: run sync or async? which is better here? In examples, they do it async (see onOpen), but why?
              ((ref.get.map(Some(_)) >>= queue.enqueue1) >> ref.set("")).toIO.unsafeRunSync()
            }
          }

          override def onBinaryFrame(payload: Array[Byte], finalFragment: Boolean, rsv: Int): Unit = {
            logger.warn(s"UNIMPLEMENTED: Tendermint WRPC: unexpected binary frame")
          }
        }
      )
      .build()

  private def connect[F[_]: Async](handler: WebSocketUpgradeHandler) =
    asyncHttpClient()
      .prepareGet(wsUrl)
      .execute(handler)
      .toCompletableFuture
      .asAsync[F]
}

object WebsocketTendermintRpc {
  private[rpc] case object Disconnected extends EffectError
}
