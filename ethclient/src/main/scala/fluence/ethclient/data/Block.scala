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

package fluence.ethclient.data

import java.math.BigInteger

import org.web3j.protocol.core.methods.response.EthBlock

case class Block(
  number: BigInt,
  hash: String,
  parentHash: String,
  nonce: String,
  sha3Uncles: String,
  logsBloom: String,
  transactionsRoot: String,
  stateRoot: String,
  receiptsRoot: String,
  author: String,
  miner: String,
  mixHash: String,
  difficulty: BigInt,
  totalDifficulty: BigInt,
  extraData: String,
  size: BigInt,
  gasLimit: BigInt,
  gasUsed: BigInt,
  timestamp: BigInt,
  transactions: Seq[Transaction],
  uncles: Seq[String],
  sealFields: Seq[String]
)

object Block {

  private def convertBigInteger(bi: BigInteger) = Option(bi).map(BigInt(_)).getOrElse(BigInt(0))

  def apply(block: EthBlock.Block): Block = {
    import block._

    import scala.collection.convert.ImplicitConversionsToScala._

    new Block(
      convertBigInteger(getNumber),
      Option(getHash).getOrElse(""),
      Option(getParentHash).getOrElse(""),
      Option(getNonceRaw).getOrElse(""), // null for kovan
      Option(getSha3Uncles).getOrElse(""),
      Option(getLogsBloom).getOrElse(""),
      Option(getTransactionsRoot).getOrElse(""),
      Option(getStateRoot).getOrElse(""),
      Option(getReceiptsRoot).getOrElse(""),
      Option(getAuthor).getOrElse(""), // empty for ganache
      Option(getMiner).getOrElse(""),
      Option(getMixHash).getOrElse(""),
      convertBigInteger(getDifficulty),
      convertBigInteger(getTotalDifficulty),
      Option(getExtraData).getOrElse(""),
      convertBigInteger(getSize),
      convertBigInteger(getGasLimit),
      convertBigInteger(getGasUsed),
      convertBigInteger(getTimestamp),
      getTransactions.toSeq.map(Transaction.apply),
      getUncles.map(Option(_).getOrElse("")),
      Option[Seq[String]](getSealFields).getOrElse(Nil) // null on ganache
    )
  }
}
