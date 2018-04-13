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

package fluence.dataset.grpc.client

import cats.effect.Effect
import cats.syntax.applicativeError._
import cats.syntax.flatMap._
import com.google.protobuf.ByteString
import fluence.btree.core.{Hash, Key}
import fluence.btree.protocol.BTreeRpc
import fluence.dataset._
import fluence.dataset.grpc.server.ServerError
import monix.eval.{MVar, Task}
import monix.execution.Scheduler
import monix.reactive.{Observable, Pipe}

import scala.collection.Searching
import scala.language.higherKinds

object Get extends slogging.LazyLogging {

  import DatasetClientOperation._

  /**
   * Initiates ''Get'' operation in remote MerkleBTree.
   *
   * @param pipe Bidi pipe for transport layer
   * @param datasetId Dataset ID
   * @param version Dataset version expected to the client
   * @param getCallbacks Wrapper for all callback needed for ''Get'' operation to the BTree
   * @return returns found value, None if nothing was found.
   */
  def apply[F[_]: Effect](
    pipe: Pipe[GetCallbackReply, GetCallback],
    datasetId: Array[Byte],
    version: Long,
    getCallbacks: BTreeRpc.SearchCallback[F]
  )(implicit sch: Scheduler): F[Option[Array[Byte]]] = {
    // Get observer/observable for request's bidiflow
    val (pushClientReply, pullServerAsk) = pipe
      .transform(_.map {
        case GetCallback(callback) ⇒
          logger.trace(s"DatasetStorageClient.get() received server ask=$callback")
          callback
      })
      .multicast

    val clientError = MVar.empty[ClientError].memoize

    /** Puts error to client error(for returning error to user of this client), and return reply with error for server.*/
    def handleClientErr(err: Throwable): F[GetCallbackReply] =
      (
        for {
          ce ← clientError
          _ ← ce.put(ClientError(err.getMessage))
        } yield GetCallbackReply(GetCallbackReply.Reply.ClientError(Error(err.getMessage)))
      ).to[F]

    val handleAsks = pullServerAsk.collect { case ask if ask.isDefined && !ask.isValue && !ask.isServerError ⇒ ask } // Collect callbacks
      .mapEval[F, GetCallbackReply] {

        case ask if ask.isNextChildIndex ⇒
          val Some(nci) = ask.nextChildIndex

          getCallbacks
            .nextChildIndex(
              nci.keys.map(k ⇒ Key(k.toByteArray)).toArray,
              nci.childsChecksums.map(c ⇒ Hash(c.toByteArray)).toArray
            )
            .attempt
            .flatMap {
              case Left(err) ⇒
                handleClientErr(err)
              case Right(idx) ⇒
                Effect[F].pure(GetCallbackReply(GetCallbackReply.Reply.NextChildIndex(ReplyNextChildIndex(idx))))
            }

        case ask if ask.isSubmitLeaf ⇒
          val Some(sl) = ask.submitLeaf

          getCallbacks
            .submitLeaf(
              sl.keys.map(k ⇒ Key(k.toByteArray)).toArray,
              sl.valuesChecksums.map(c ⇒ Hash(c.toByteArray)).toArray
            )
            .attempt
            .flatMap {
              case Left(err) ⇒
                handleClientErr(err)
              case Right(searchResult) ⇒
                Effect[F].pure(
                  GetCallbackReply(
                    GetCallbackReply.Reply.SubmitLeaf(
                      ReplySubmitLeaf(
                        searchResult match {
                          case Searching.Found(i) ⇒ ReplySubmitLeaf.SearchResult.Found(i)
                          case Searching.InsertionPoint(i) ⇒ ReplySubmitLeaf.SearchResult.InsertionPoint(i)
                        }
                      )
                    )
                  )
                )
            }

      }

    (
      Observable(
        GetCallbackReply(
          GetCallbackReply.Reply.DatasetInfo(DatasetInfo(ByteString.copyFrom(datasetId), version))
        )
      ) ++ handleAsks
    ).subscribe(pushClientReply) // And clientReply response back to server

    val serverErrOrVal =
      pullServerAsk.collect { // Collect terminal task with value/error
        case ask if ask.isServerError ⇒
          val Some(err) = ask.serverError
          val serverError = ServerError(err.msg)
          // if server send an error we should close stream and lift error up
          Task(pushClientReply.onError(serverError))
            .flatMap(_ ⇒ Task.raiseError[Option[Array[Byte]]](serverError))
        case ask if ask.isValue ⇒
          val Some(getValue) = ask._value
          // if got success response or server error close stream and return value\error to user of this client
          Task(pushClientReply.onComplete()).map { _ ⇒
            Option(getValue.value)
              .filterNot(_.isEmpty)
              .map(_.toByteArray)
          }
      }.headOptionL // Take the first option value or server error

    composeResult(clientError, serverErrOrVal)

  }
}
