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

package fluence.statemachine

import cats.effect.Effect
import cats.effect.concurrent.Ref
import cats.effect.syntax.effect._
import cats.syntax.applicative._
import cats.syntax.flatMap._
import cats.syntax.functor._
import cats.syntax.option._
import cats.{Applicative, Monad}
import com.github.jtendermint.jabci.api.CodeType
import fluence.crypto.Crypto
import fluence.crypto.Crypto.Hasher
import fluence.crypto.hash.JdkCryptoHasher
import fluence.effects.tendermint.block.TendermintBlock
import fluence.effects.tendermint.block.errors.Errors._
import fluence.effects.tendermint.rpc.http.TendermintHttpRpc
import fluence.statemachine.control.{BlockReceipt, ControlSignals, ReceiptType}
import fluence.log.Log
import fluence.statemachine.control.ControlSignals
import fluence.statemachine.state.AbciState
import scodec.bits.ByteVector

import scala.language.higherKinds

/**
 * Wraps all the state and logic required to perform ABCI logic.
 *
 * @param state See [[AbciState]]
 * @param vm Virtual machine invoker
 * @param controlSignals Communication channel with master node
 */
class AbciService[F[_]: Monad: Effect](
  state: Ref[F, AbciState],
  vm: VmOperationInvoker[F],
  controlSignals: ControlSignals[F],
  tendermintRpc: TendermintHttpRpc[F]
)(implicit hasher: Hasher[ByteVector, ByteVector]) {

  import AbciService._

  private def checkBlock(height: Long): Unit = {
    val log: String ⇒ Unit = s ⇒ logger.info(Console.YELLOW + s + Console.RESET)
    val logBad: String ⇒ Unit = s ⇒ logger.info(Console.RED + s + Console.RESET)

    tendermintRpc
      .block(height)
      .value
      .toIO
      .map(
        blockE =>
          for {
            block <- blockE.leftTap(e => logger.warn(s"RPC Block[$height] failed: $e ${e.getCause}"))
            _ = logger.info(s"RPC Block[$height] => height = ${block.header.height}")
            _ <- TendermintBlock(block)
              .validateHashes()
              .leftTap(e => logBad(s"Block at height $height is invalid: $e ${e.getCause}"))
          } yield log(s"Block at height $height is valid")
      )
      .unsafeRunAsyncAndForget()
  }

  /**
   * Take all the transactions we're able to process, and pass them to VM one by one.
   *
   * @return App (VM) Hash
   */
  def commit(implicit log: Log[F]): F[ByteVector] =
    for {
      // Get current state
      s ← state.get
      // Form a block: take ordered txs from AbciState
      sTxs @ (_, transactions) ← AbciState.formBlock[F].run(s)

      // Process txs one by one
      st ← Monad[F].tailRecM[(AbciState, List[Tx]), AbciState](sTxs) {
        case (st, tx :: txs) ⇒
          // Invoke
          vm.invoke(tx.data.value)
            // Save the tx response to AbciState
            .semiflatMap(value ⇒ AbciState.putResponse[F](tx.head, value).map(_ ⇒ txs).run(st).map(Left(_)))
            .leftSemiflatMap(err ⇒ Log[F].error(s"VM invoke failed: $err for tx: $tx").as(err))
            .getOrElse(Right(st)) // TODO do not ignore vm error

        case (st, Nil) ⇒
          Applicative[F].pure(Right(st))
      }

      // Get the VM hash
      vmHash ← vm
        .vmStateHash()
        .leftSemiflatMap(err ⇒ Log[F].error(s"VM is unable to compute state hash: $err").as(err))
        .getOrElse(ByteVector.empty) // TODO do not ignore vm error

      // Do not wait for receipt on the very first block
      receipt <- if (st.height > 0) controlSignals.receipt.map(_.some) else none[BlockReceipt].pure[F]
      _ = receipt.foreach(
        b =>
          if (b.receipt.height != st.height)
            logger.error(s"Got wrong receipt height. st.height: ${st.height}, receipt: ${b.receipt.height}")
      )

      // Check block for correctness, for debugging purposes
      _ = if (st.height > 0 && receipt.forall(_.`type` == ReceiptType.New)) checkBlock(st.height)

      // Do not use receipt in app hash if there's no txs in a block, so empty blocks have the same appHash as
      // previous non-empty ones. This is because Tendermint stops producing empty blocks only after
      // at least 2 blocks have the same appHash. Otherwise, empty blocks would be produced indefinitely.
      // TODO: use appHash for the previous block instead of vmHash.pure[F]
      appHash <- receipt.filter(_ => transactions.nonEmpty).fold(vmHash.pure[F]) {
        case BlockReceipt(r, _) =>
          hasher(vmHash ++ r.bytes())
            .leftMap(err => logger.error(s"Error on hashing vmHash + receipt: $err"))
            .getOrElse(vmHash) // TODO: that's awful; don't ignore errors
      }

      // Push hash to AbciState, increment block number
      newState ← AbciState.setAppHash(appHash).runS(st)

      // Store updated state in the Ref (the changes were transient for readers before this step)
      _ ← state.set(newState)

      // Store vmHash, so master node could retrieve it
      _ <- receipt.map(_.`type`) match {
        // Most common case. Receipt for a previous block; None means block was either empty or the very first one
        case Some(ReceiptType.New) | None => controlSignals.putVmHash(vmHash)
        // After-restart case. Node has stored receipts, and replaying them. There will be no demand for vmHash, so skip it
        case Some(ReceiptType.Stored) => Applicative[F].unit
        // After-restart case. Last of the stored receipts, so vmHash is new, and will be required for the next block manifest
        case Some(ReceiptType.LastStored) => controlSignals.setVmHash(vmHash)
        case unknown                      => logger.error(s"Unknown receipt kind: $unknown").pure[F]
      }
    } yield appHash

  /**
   * Queries the storage for sessionId/nonce result, or for sessionId status.
   *
   * @param path sessionId/nonce or sessionId
   */
  def query(path: String): F[QueryResponse] =
    Tx.readHead(path) match {
      // There's no /nonce part, but path could be a sessionId as a whole
      case None ⇒
        state.get.map(
          state ⇒
            QueryResponse(
              state.height,
              Array.emptyByteArray,
              Codes.NotFound,
              s"Cannot parse query path: $path, must be in `sessionId/nonce` format"
          )
        )

      case Some(head) ⇒
        // It's a query for a particular response for a session and nonce
        state.get.map { st ⇒
          st.responses.find(_._1 == head) match {
            case Some((_, data)) ⇒
              QueryResponse(st.height, data, Codes.Ok, s"Responded for path $path")

            case _ ⇒
              // Is it pending or unknown?
              if (st.sessions.data.get(head.session).exists(_.nextNonce <= head.nonce))
                QueryResponse(
                  st.height,
                  Array.emptyByteArray,
                  Codes.Pending,
                  s"Transaction is not yet processed: $path"
                )
              else
                QueryResponse(
                  st.height,
                  Array.emptyByteArray,
                  Codes.NotFound,
                  s"No response found for path: $path"
                )
          }
        }
    }

  /**
   * Push incoming transaction to be processed on [[commit]].
   *
   * @param data Incoming transaction
   */
  def deliverTx(data: Array[Byte])(implicit log: Log[F]): F[TxResponse] =
    Tx.readTx(data).value.flatMap {
      case Some(tx) ⇒
        // TODO we have different logic in checkTx and deliverTx, as only in deliverTx tx might be dropped due to pending txs overflow
        state
        // Update the state with a new tx
          .modifyState(AbciState.addTx(tx))
          .map {
            case true ⇒ TxResponse(CodeType.OK, s"Delivered\n${tx.head}")
            case false ⇒ TxResponse(CodeType.BadNonce, s"Dropped\n${tx.head}")
          }
      case None ⇒
        Applicative[F].pure(TxResponse(CodeType.BAD, s"Cannot parse transaction header"))
    }

  /**
   * Check if transaction is well-formed: [[Tx.readTx()]] must return Some
   *
   * @param data Incoming transaction
   */
  def checkTx(data: Array[Byte])(implicit log: Log[F]): F[TxResponse] =
    Tx.readTx(data).value.flatMap {
      case Some(tx) ⇒
        state.get
          .map(
            !_.sessions.data
              .get(tx.head.session)
              .exists(_.nextNonce > tx.head.nonce)
          )
          .map {
            case true ⇒
              // Session is unknown, or nonce is valid
              TxResponse(CodeType.OK, s"Parsed transaction head: ${tx.head}")
            case false ⇒
              // Invalid nonce -- misorder
              TxResponse(CodeType.BadNonce, s"Misordered\n${tx.head}")
          }
      case None ⇒
        Applicative[F].pure(TxResponse(CodeType.BAD, s"Cannot parse transaction header"))
    }
}

object AbciService {

  object Codes {
    val Ok: Int = 0
    val CannotParseHeader: Int = 1
    val Dropped: Int = 2
    val NotFound: Int = 3
    val Pending: Int = 4
  }

  /**
   * A structure for aggregating data specific to building `Query` ABCI method response.
   *
   * @param height height corresponding to state for which result given
   * @param result requested result, if found
   * @param code response code
   * @param info response message
   */
  case class QueryResponse(height: Long, result: Array[Byte], code: Int, info: String)

  /**
   * A structure for aggregating data specific to building `CheckTx`/`DeliverTx` ABCI response.
   *
   * @param code response code
   * @param info response message
   */
  case class TxResponse(code: Int, info: String)

  /**
   * Build an empty AbciService for the vm. App hash is empty!
   *
   * @param vm VM to invoke
   * @tparam F Sync for Ref
   * @return Brand new AbciService instance
   */
  def apply[F[_]: Effect](
    vm: VmOperationInvoker[F],
    controlSignals: ControlSignals[F],
    tendermintRpc: TendermintHttpRpc[F]
  ): F[AbciService[F]] = {
    import cats.syntax.compose._
    import scodec.bits.ByteVector

    import scala.language.higherKinds

    for {
      state ← Ref.of[F, AbciState](AbciState())
    } yield {

      val bva = Crypto.liftFunc[ByteVector, Array[Byte]](_.toArray)
      val abv = Crypto.liftFunc[Array[Byte], ByteVector](ByteVector(_))
      implicit val hasher: Crypto.Hasher[ByteVector, ByteVector] =
        bva.andThen[Array[Byte]](JdkCryptoHasher.Sha256).andThen(abv)

      new AbciService[F](state, vm, controlSignals, tendermintRpc)
    }
  }

}
