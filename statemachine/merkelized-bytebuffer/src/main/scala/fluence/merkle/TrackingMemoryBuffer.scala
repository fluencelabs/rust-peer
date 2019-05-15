package fluence.merkle

import java.nio.{ByteBuffer, ByteOrder}
import java.util

import asmble.compile.jvm.MemoryBuffer

/**
 * Wrapper for `ByteBuffer` with tracking all changes in `ByteBuffer` and marking these changes in BitSet.
 * Uses with `BinaryMerkleTree` to recalculate hash only for changed bytes.
 *
 * @param bb wrapped bytes
 * @param chunkSize size of one chunk that will be marked as changed if one or more bytes will be changed in this chunk
 */
class TrackingMemoryBuffer(val bb: ByteBuffer, val chunkSize: Int) extends MemoryBuffer {
  import TrackingMemoryBuffer._

  private val size = bb.capacity()

  private val dirtyChunks = new util.BitSet(size / chunkSize)

  /**
   * Finds a chunk where byte with `index` is presented.
   *
   * @param index of byte
   * @return a chunk of bytes
   */
  def getChunk(index: Int): ByteBuffer = {
    val duplicated = bb.duplicate()
    duplicated.order(ByteOrder.LITTLE_ENDIAN)
    duplicated.clear()
    duplicated.position(index)
    duplicated.limit(index + chunkSize)
    duplicated
  }

  /**
   * Returns list of pointers on dirty chunks encoded in BitSet for compaction.
   *
   */
  def getDirtyChunks: util.BitSet = dirtyChunks

  def slice(): ByteBuffer = bb.slice()

  def duplicate(): MemoryBuffer = {
    new TrackingMemoryBuffer(bb.duplicate(), chunkSize)
  }

  def get(): Byte = bb.get()

  def get(dst: Array[Byte], offset: Int, length: Int): Unit = {
    bb.get(dst, offset, length)
  }

  def put(b: Byte): TrackingMemoryBuffer = {
    val index = bb.position()
    bb.put(b)
    dirtyChunks.set(index / chunkSize)
    this
  }

  def get(index: Int): Byte = bb.get(index)

  def put(index: Int, b: Byte): TrackingMemoryBuffer = {
    bb.put(index, b)
    getDirtyChunks.set(index / chunkSize)
    this
  }

  def getShort(index: Int): Short = bb.getShort(index)

  def putShort(index: Int, value: Short): MemoryBuffer = {
    bb.putShort(index, value)
    val from = index / chunkSize
    val to = (index + SHORT_SIZE) / chunkSize
    if (from == to) getDirtyChunks.set(from)
    else getDirtyChunks.set(from, to)
    this
  }

  def capacity: Int = bb.capacity

  def clear(): MemoryBuffer = {
    bb.clear
    this
  }

  def limit: Int = bb.limit

  def limit(newLimit: Int): MemoryBuffer = {
    bb.limit(newLimit)
    this
  }

  def position: Int = bb.position

  def position(newPosition: Int): MemoryBuffer = {
    bb.position(newPosition)
    this
  }

  override def order(order: ByteOrder): MemoryBuffer = {
    bb.order(order)
    this
  }

  override def put(arr: Array[Byte], offset: Int, length: Int): MemoryBuffer = {
    bb.put(arr, offset, length)
    getDirtyChunks.set(offset / chunkSize, (offset + length) / chunkSize)
    this
  }

  override def put(arr: Array[Byte]): MemoryBuffer = {
    val pos = bb.position()
    bb.put(arr)
    getDirtyChunks.set(pos / chunkSize, (pos + arr.length) / chunkSize)
    this
  }

  override def get(arr: Array[Byte]): MemoryBuffer = {
    bb.get(arr)
    this
  }

  override def putInt(index: Int, n: Int): MemoryBuffer = {
    bb.putInt(index, n)
    val from = index / chunkSize
    val to = (index + INT_SIZE) / chunkSize
    if (from == to) getDirtyChunks.set(from)
    else getDirtyChunks.set(from, to)
    this
  }

  override def putLong(index: Int, n: Long): MemoryBuffer = {
    bb.putLong(index, n)
    val from = index / chunkSize
    val to = (index + LONG_SIZE) / chunkSize
    if (from == to) getDirtyChunks.set(from)
    else getDirtyChunks.set(from, to)
    this
  }

  override def putDouble(index: Int, n: Double): MemoryBuffer = {
    bb.putDouble(index, n)
    val from = index / chunkSize
    val to = (index + DOUBLE_SIZE) / chunkSize
    if (from == to) getDirtyChunks.set(from)
    else getDirtyChunks.set(from, to)
    this
  }

  override def putFloat(index: Int, n: Float): MemoryBuffer = {
    bb.putFloat(index, n)
    val from = index / chunkSize
    val to = (index + FLOAT_SIZE) / chunkSize
    if (from == to) getDirtyChunks.set(from)
    else getDirtyChunks.set(from, to)
    this
  }

  override def getInt(index: Int): Int = bb.getInt(index)

  override def getLong(index: Int): Long = bb.getLong(index)

  override def getFloat(index: Int): Float = bb.getFloat(index)

  override def getDouble(index: Int): Double = bb.getDouble(index)
}

object TrackingMemoryBuffer {

  val SHORT_SIZE = 2
  val INT_SIZE = 4
  val LONG_SIZE = 8
  val FLOAT_SIZE = 4
  val DOUBLE_SIZE = 8

  /**
   * Creates TrackingMemoryBuffer that wrapping HeapByteBuffer.
   *
   * @param capacity the new buffer's capacity, in bytes
   * @param chunkSize size of one chunk that will be marked as changed if one or more bytes will be changed in this chunk
   */
  def allocate(capacity: Int, chunkSize: Int): TrackingMemoryBuffer = {
    new TrackingMemoryBuffer(ByteBuffer.allocate(capacity), chunkSize)
  }

  /**
   * Creates TrackingMemoryBuffer that wrapping DirectByteBuffer.
   *
   * @param capacity the new buffer's capacity, in bytes
   * @param chunkSize size of one chunk that will be marked as changed if one or more bytes will be changed in this chunk
   */
  def allocateDirect(capacity: Int, chunkSize: Int): TrackingMemoryBuffer = {
    new TrackingMemoryBuffer(ByteBuffer.allocateDirect(capacity), chunkSize)
  }
}
