package fluence.node.workers.subscription

import cats.{Monad, Traverse}
import cats.effect.{Concurrent, Resource, Timer}
import cats.effect.concurrent.Ref
import fluence.statemachine.data.Tx
import cats.syntax.functor._
import cats.syntax.flatMap._
import cats.syntax.applicative._
import fluence.crypto.Crypto.Hasher
import fluence.effects.tendermint.rpc.http.TendermintHttpRpc
import fluence.effects.tendermint.rpc.websocket.TendermintWebsocketRpc
import fluence.effects.{Backoff, EffectError}
import fluence.log.Log
import fluence.node.MakeResource
import fluence.node.workers.subscription.StoredProcedureExecutor.TendermintResponse
import fs2.concurrent.{NoneTerminatedQueue, Queue}

import scala.language.higherKinds
import scala.util.Random

case class SubscriptionState[F[_]](
  tx: Tx.Data,
  queue: NoneTerminatedQueue[F, Either[TxAwaitError, TendermintQueryResponse]],
  output: fs2.Stream[F, fs2.Stream[F, Either[TxAwaitError, TendermintQueryResponse]]],
  subNumber: Int
)

class StoredProcedureExecutorImpl[F[_]: Monad: Timer](
  subscriptions: Ref[F, Map[String, SubscriptionState[F]]],
  tendermintWRpc: TendermintWebsocketRpc[F],
  tendermintRpc: TendermintHttpRpc[F],
  waitResponseService: WaitResponseService[F],
  hasher: Hasher[Array[Byte], String]
)(
  implicit backoff: Backoff[EffectError] = Backoff.default[EffectError],
  F: Concurrent[F],
  log: Log[F]
) extends StoredProcedureExecutor[F] {

  /**
   * Makes a subscription by transaction.
   * The master node will send a transaction to state machine after every block
   * and will return response to a connected client.
   *
   * @param data a transaction
   * @return a stream of responses every block
   */
  override def subscribe(data: Tx.Data): F[fs2.Stream[F, Option[TendermintResponse]]] = {
    for {
      q <- Queue.noneTerminated[F, Either[TxAwaitError, TendermintQueryResponse]]
      key = hasher.unsafe(data.value)
      output <- subscriptions.modify { subs =>
        subs.get(key) match {
          case Some(sub) => (subs.updated(key, sub.copy(subNumber = sub.subNumber + 1)), sub.output)
          case None =>
            val newState = SubscriptionState(data, q, q.dequeue.broadcast, 1)
            (subs + (key -> newState), newState.output)
        }
      }
    } yield output.take(1).flatten.noneTerminate
  }

  override def unsubscribe(data: Tx.Data): F[Boolean] = {
    val key = hasher.unsafe(data.value)
    for {
      (isOk, queueToClose) <- subscriptions.modify { subs =>
        subs.get(key) match {
          case Some(sub) =>
            val updated =
              if (sub.subNumber == 1) subs - key
              else subs.updated(key, sub.copy(subNumber = sub.subNumber - 1))
            (updated, (true, if (sub.subNumber == 1) Option(sub.queue) else None))
          case None => (subs, (false, None))
        }
      }
      _ <- queueToClose match {
        case Some(q) => q.enqueue1(None)
        case None    => ().pure[F]
      }
    } yield isOk

  }

  /**
   * Gets all transaction subscribes for appId and trying to poll service for new responses.
   *
   */
  override def start(): Resource[F, Unit] =
    log.scope("stateSubscriber") { implicit log =>
      for {
        lastHeight <- Resource.liftF(
          backoff.retry(tendermintRpc.consensusHeight(), e => log.error("retrieving consensus height", e))
        )
        _ <- Log.resource.info("Creating subscription for tendermint blocks")
        blockStream = tendermintWRpc.subscribeNewBlock(lastHeight)
        pollingStream = blockStream
          .evalTap(b => log.debug(s"got block ${b.header.height}"))
          .evalMap(_ => processSubsribes())
        _ <- MakeResource.concurrentStream(pollingStream)
      } yield ()
    }

  private def processSubsribes() = {
    import cats.instances.list._
    for {
      subs <- subscriptions.get
      _ = println("subscriptions: " + subs)
      tasks = subs.map {
        case (key, SubscriptionState(data, queue, _, _)) =>
          val randomStr = Random.alphanumeric.take(8).mkString
          val head = Tx.Head(s"pubsub-$key-$randomStr", 0)
          val tx = Tx(head, data)

          println("tx: " + tx)

          for {
            response <- waitResponseService.sendTxAwaitResponse(tx.generateTx(), None)
            _ = println("response: " + response)
            _ <- queue.enqueue1(Some(response))
          } yield {}
      }
      _ <- Traverse[List].traverse(tasks.toList)(F.start)
    } yield ()
  }
}
