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

package fluence.dataset.grpc.server

import cats.data.EitherT
import cats.effect.Async
import cats.syntax.applicativeError._
import cats.syntax.flatMap._
import cats.syntax.functor._
import cats.syntax.show._
import cats.{~>, Monad}
import com.google.protobuf.ByteString
import fluence.btree.core.{ClientPutDetails, Hash, Key}
import fluence.btree.protocol.BTreeRpc
import fluence.dataset._
import fluence.dataset.grpc.GrpcMonix._
import fluence.dataset.grpc.client.ClientError
import fluence.dataset.protocol.DatasetStorageRpc
import fluence.dataset.service.DatasetStorageRpcGrpc
import io.grpc.stub.StreamObserver
import monix.eval.Task
import monix.execution.{Ack, Scheduler}
import monix.reactive.{Observable, Observer}
import scodec.bits.ByteVector

import scala.collection.Searching
import scala.language.higherKinds

/**
 * Server implementation of [[DatasetStorageRpcGrpc.DatasetStorageRpc]], allows talking to client via network.
 * All public methods called from the server side.
 * DatasetStorageServer is active and initiates requests to client.
 *
 * @param service Server implementation of [[DatasetStorageRpc]] to which the calls will be delegated
 * @tparam F A box for returning value
 */
class DatasetStorageServer[F[_]: Async](
  service: DatasetStorageRpc[F, Observable]
)(
  implicit
  F: Monad[F],
  runF: F ~> Task,
  scheduler: Scheduler
) extends DatasetStorageRpcGrpc.DatasetStorageRpc with slogging.LazyLogging {
  import DatasetServerOperation._

  override def get(responseObserver: StreamObserver[GetCallback]): StreamObserver[GetCallbackReply] = {

    val resp: Observer[GetCallback] = responseObserver
    val (repl: Observable[GetCallbackReply], stream) = streamObservable[GetCallbackReply]
    val pullClientReply: () ⇒ Task[GetCallbackReply] = repl.pullable

    Get(service, resp, repl, pullClientReply)

    stream
  }

  override def range(responseObserver: StreamObserver[RangeCallback]): StreamObserver[RangeCallbackReply] = {

    val resp: Observer[RangeCallback] = responseObserver
    val (repl, stream) = streamObservable[RangeCallbackReply]
    val pullClientReply = repl.pullable

    def getReply[T](
      check: RangeCallbackReply.Reply ⇒ Boolean,
      extract: RangeCallbackReply.Reply ⇒ T
    ): EitherT[Task, ClientError, T] = {

      val clReply = pullClientReply().map {
        case RangeCallbackReply(reply) ⇒
          logger.trace(s"DatasetStorageServer.range() received client reply=$reply")
          reply
      }.map {
        case r if check(r) ⇒
          Right(extract(r))
        case r ⇒
          val errMsg = r.clientError.map(_.msg).getOrElse("Wrong reply received, protocol error")
          Left(ClientError(errMsg))
      }

      EitherT(clReply)
    }

    val valueF =
      for {
        datasetInfo ← toObservable(getReply(_.isDatasetInfo, _.datasetInfo.get))
        valuesStream ← service.range(
          datasetInfo.id.toByteArray,
          datasetInfo.version,
          new BTreeRpc.SearchCallback[F] {

            private val pushServerAsk: RangeCallback.Callback ⇒ EitherT[Task, ClientError, Ack] = callback ⇒ {
              EitherT(Task.fromFuture(resp.onNext(RangeCallback(callback = callback))).attempt)
                .leftMap(t ⇒ ClientError(t.getMessage))
            }

            /**
             * Server sends founded leaf details.
             *
             * @param keys            Keys of current leaf
             * @param valuesChecksums Checksums of values for current leaf
             * @return index of searched value, or None if key wasn't found
             */
            override def submitLeaf(keys: Array[Key], valuesChecksums: Array[Hash]): F[Searching.SearchResult] =
              toF(
                for {
                  _ ← pushServerAsk(
                    RangeCallback.Callback.SubmitLeaf(
                      AskSubmitLeaf(
                        keys = keys.map(k ⇒ ByteString.copyFrom(k.bytes)),
                        valuesChecksums = valuesChecksums.map(c ⇒ ByteString.copyFrom(c.bytes))
                      )
                    )
                  )
                  sl ← getReply(_.isSubmitLeaf, _.submitLeaf.get)
                } yield {
                  sl.searchResult.found
                    .map(Searching.Found)
                    .orElse(sl.searchResult.insertionPoint.map(Searching.InsertionPoint))
                    .get
                }
              )

            /**
             * Server asks next child node index.
             *
             * @param keys            Keys of current branch for searching index
             * @param childsChecksums All children checksums of current branch
             */
            override def nextChildIndex(keys: Array[Key], childsChecksums: Array[Hash]): F[Int] =
              toF(
                for {
                  _ ← pushServerAsk(
                    RangeCallback.Callback.NextChildIndex(
                      AskNextChildIndex(
                        keys = keys.map(k ⇒ ByteString.copyFrom(k.bytes)),
                        childsChecksums = childsChecksums.map(c ⇒ ByteString.copyFrom(c.bytes))
                      )
                    )
                  )
                  nci ← getReply(_.isNextChildIndex, _.nextChildIndex.get)
                } yield nci.index
              )
          }
        )
      } yield {
        logger.debug(s"Was found value=${valuesStream.show} for client 'range' request for ${datasetInfo.show}")
        valuesStream
      }

    valueF.attempt.flatMap {
      case Right((key, value)) ⇒
        // if all is ok server should send value to client
        Observable.pure(
          RangeCallback(RangeCallback.Callback.Value(RangeValue(ByteString.copyFrom(key), ByteString.copyFrom(value))))
        )
      case Left(clientError: ClientError) ⇒
        logger.warn(s"Client replied with an error=$clientError")
        // if server receive client error server should lift it up
        Observable.raiseError(clientError)
      case Left(exception) ⇒
        // when server error appears, server should log it and send to client
        logger.warn(s"Server threw an exception=$exception and sends cause to client")
        Observable.pure(RangeCallback(RangeCallback.Callback.ServerError(Error(exception.getMessage))))
    }.subscribe(resp)

    stream
  }

  override def put(responseObserver: StreamObserver[PutCallback]): StreamObserver[PutCallbackReply] = {
    val resp: Observer[PutCallback] = responseObserver
    val (repl, stream) = streamObservable[PutCallbackReply]
    val pullClientReply = repl.pullable

    def getReply[T](
      check: PutCallbackReply.Reply ⇒ Boolean,
      extract: PutCallbackReply.Reply ⇒ T
    ): EitherT[Task, ClientError, T] = {

      val reply = pullClientReply().map {
        case PutCallbackReply(r) ⇒
          logger.trace(s"DatasetStorageServer.put() received client reply=$r")
          r
      }.map {
        case r if check(r) ⇒
          Right(extract(r))
        case r ⇒
          val errMsg = r.clientError.map(_.msg).getOrElse("Wrong reply received, protocol error")
          Left(ClientError(errMsg))
      }

      EitherT(reply)
    }

    val valueF =
      for {
        datasetInfo ← toF(getReply(_.isDatasetInfo, _.datasetInfo.get))
        putValue ← toF(getReply(_.isValue, _._value.map(_.value.toByteArray).getOrElse(Array.emptyByteArray)))
        oldValue ← service.put(
          datasetInfo.id.toByteArray,
          datasetInfo.version,
          new BTreeRpc.PutCallbacks[F] {

            private val pushServerAsk: PutCallback.Callback ⇒ EitherT[Task, ClientError, Ack] = callback ⇒ {
              EitherT(Task.fromFuture(resp.onNext(PutCallback(callback = callback))).attempt)
                .leftMap(t ⇒ ClientError(t.getMessage))
            }

            /**
             * Server asks next child node index.
             *
             * @param keys            Keys of current branch for searching index
             * @param childsChecksums All children checksums of current branch
             */
            override def nextChildIndex(keys: Array[Key], childsChecksums: Array[Hash]): F[Int] =
              toF(
                for {
                  _ ← pushServerAsk(
                    PutCallback.Callback.NextChildIndex(
                      AskNextChildIndex(
                        keys = keys.map(k ⇒ ByteString.copyFrom(k.bytes)),
                        childsChecksums = childsChecksums.map(c ⇒ ByteString.copyFrom(c.bytes))
                      )
                    )
                  )
                  nci ← getReply(_.isNextChildIndex, _.nextChildIndex.get)
                } yield nci.index
              )

            /**
             * Server sends founded leaf details.
             *
             * @param keys            Keys of current leaf
             * @param valuesChecksums Checksums of values for current leaf
             */
            override def putDetails(keys: Array[Key], valuesChecksums: Array[Hash]): F[ClientPutDetails] =
              toF(
                for {
                  _ ← pushServerAsk(
                    PutCallback.Callback.PutDetails(
                      AskPutDetails(
                        keys = keys.map(k ⇒ ByteString.copyFrom(k.bytes)),
                        valuesChecksums = valuesChecksums.map(c ⇒ ByteString.copyFrom(c.bytes))
                      )
                    )
                  )
                  pd ← getReply(r ⇒ r.isPutDetails && r.putDetails.exists(_.searchResult.isDefined), _.putDetails.get)
                } yield
                  ClientPutDetails(
                    key = Key(pd.key.toByteArray),
                    valChecksum = Hash(pd.checksum.toByteArray),
                    searchResult = (
                      pd.searchResult.found.map(Searching.Found) orElse
                        pd.searchResult.insertionPoint.map(Searching.InsertionPoint)
                    ).get
                  )
              )

            /**
             * Server sends new merkle root to client for approve made changes.
             *
             * @param serverMerkleRoot New merkle root after putting key/value
             * @param wasSplitting     'True' id server performed tree rebalancing, 'False' otherwise
             */
            override def verifyChanges(serverMerkleRoot: Hash, wasSplitting: Boolean): F[ByteVector] =
              toF(
                for {
                  _ ← pushServerAsk(
                    PutCallback.Callback.VerifyChanges(
                      AskVerifyChanges(
                        serverMerkleRoot = ByteString.copyFrom(serverMerkleRoot.bytes),
                        splitted = wasSplitting
                      )
                    )
                  )
                  clientSignature ← getReply(_.isVerifyChanges, _.verifyChanges.get.signature)
                } yield ByteVector(clientSignature.toByteArray)
              )

            /**
             * Server confirms that all changes was persisted.
             */
            override def changesStored(): F[Unit] =
              toF(
                for {
                  _ ← pushServerAsk(PutCallback.Callback.ChangesStored(AskChangesStored()))
                  _ ← getReply(_.isChangesStored, _.changesStored.get)
                } yield ()
              )
          },
          putValue
        )
      } yield {
        logger.debug(
          s"Was stored new value=${putValue.show} for client 'put' request for ${datasetInfo.show}" +
            s" old value=${oldValue.show} was overwritten"
        )
        oldValue
      }

    // Launch service call, push the value once it's received
    resp completeWith runF(
      valueF.attempt.flatMap {
        case Right(value) ⇒
          // if all is ok server should close the stream(is done in ObserverGrpcOps.completeWith)  and send value to client
          Async[F].pure(
            PutCallback(PutCallback.Callback.Value(PreviousValue(value.fold(ByteString.EMPTY)(ByteString.copyFrom))))
          )
        case Left(clientError: ClientError) ⇒
          // when server receive client error, server shouldn't close the stream(is done in ObserverGrpcOps.completeWith)  and lift up client error
          Async[F].raiseError[PutCallback](clientError)
        case Left(exception) ⇒
          // when server error appears, server should log it and send to client
          logger.warn(s"Severs throw an exception=($exception) and send cause to client")
          F.pure(PutCallback(PutCallback.Callback.ServerError(Error(exception.getMessage))))
      }
    )

    stream
  }
}
